//! commit 对象与签名行。
//!
//! **T1 待实现范围**：`Signature::parse/format`、`Commit::encode_payload/decode_payload`。
//!
//! 格式（顺序固定，未知扩展头必须原样保留）：
//! ```text
//! tree <oid>\n
//! parent <oid>\n        (0..n 行，首父在前)
//! author Name <email> <unix-ts> <±HHMM>\n
//! committer Name <email> <unix-ts> <±HHMM>\n
//! <扩展头，多行用前导空格续行>\n
//! \n
//! <message>
//! ```
//! 验收：`git log -1 --format=raw` 与 `mg cat-file -p HEAD` 输出一致。

use crate::error::{Error, Result};
use crate::oid::Oid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub name: Vec<u8>,
    pub email: Vec<u8>,
    /// Unix 时间戳（秒）。
    pub when: i64,
    /// 形如 `+0800`。
    pub tz: String,
}

impl Signature {
    pub fn new(
        name: impl Into<Vec<u8>>,
        email: impl Into<Vec<u8>>,
        when: i64,
        tz: impl Into<String>,
    ) -> Self {
        Signature {
            name: name.into(),
            email: email.into(),
            when,
            tz: tz.into(),
        }
    }

    /// 解析 `Name <email> <unix-ts> <±HHMM>`（git 的 `split_ident_line` 规则：
    /// name 是 `<` 之前去掉尾部空白的内容）。
    pub fn parse(input: &[u8]) -> Result<Self> {
        let bad = |detail: &str| {
            Error::corrupt(
                "signature",
                format!("{detail} in {:?}", String::from_utf8_lossy(input)),
            )
        };

        let open = input
            .iter()
            .position(|b| *b == b'<')
            .ok_or_else(|| bad("missing '<'"))?;
        let close = open
            + 1
            + input[open + 1..]
                .iter()
                .position(|b| *b == b'>')
                .ok_or_else(|| bad("missing '>'"))?;

        let mut name_end = open;
        while name_end > 0 && input[name_end - 1].is_ascii_whitespace() {
            name_end -= 1;
        }

        let mut rest = skip_whitespace(&input[close + 1..]);
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 {
            return Err(bad("missing timestamp"));
        }
        let when: i64 = std::str::from_utf8(&rest[..digits])
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or_else(|| bad("timestamp out of range"))?;

        rest = skip_whitespace(&rest[digits..]);
        if rest.len() != 5
            || !(rest[0] == b'+' || rest[0] == b'-')
            || !rest[1..].iter().all(u8::is_ascii_digit)
        {
            return Err(bad("expected a ±HHMM timezone"));
        }

        Ok(Signature {
            name: input[..name_end].to_vec(),
            email: input[open + 1..close].to_vec(),
            when,
            tz: String::from_utf8_lossy(rest).into_owned(),
        })
    }

    /// `Name <email> <unix-ts> <±HHMM>`；与 [`Signature::parse`] 逐字节互逆。
    pub fn format(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.name.len() + self.email.len() + 32);
        out.extend_from_slice(&self.name);
        out.extend_from_slice(b" <");
        out.extend_from_slice(&self.email);
        out.extend_from_slice(b"> ");
        out.extend_from_slice(self.when.to_string().as_bytes());
        out.push(b' ');
        out.extend_from_slice(self.tz.as_bytes());
        out
    }
}

fn skip_whitespace(input: &[u8]) -> &[u8] {
    let len = input.iter().take_while(|b| b.is_ascii_whitespace()).count();
    &input[len..]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub tree: Oid,
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    pub message: Vec<u8>,
    /// 形如 `("gpgsig", <原样字节>)`；编码时按读入顺序写回。
    pub extra_headers: Vec<(String, Vec<u8>)>,
}

impl Commit {
    pub fn encode_payload(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"tree ");
        out.extend_from_slice(self.tree.to_hex().as_bytes());
        out.push(b'\n');
        for parent in &self.parents {
            out.extend_from_slice(b"parent ");
            out.extend_from_slice(parent.to_hex().as_bytes());
            out.push(b'\n');
        }
        out.extend_from_slice(b"author ");
        out.extend_from_slice(&self.author.format());
        out.push(b'\n');
        out.extend_from_slice(b"committer ");
        out.extend_from_slice(&self.committer.format());
        out.push(b'\n');
        for (key, value) in &self.extra_headers {
            out.extend_from_slice(key.as_bytes());
            out.push(b' ');
            out.extend_from_slice(value);
            out.push(b'\n');
        }
        out.push(b'\n');
        out.extend_from_slice(&self.message);
        out
    }

    pub fn decode_payload(payload: &[u8]) -> Result<Self> {
        let (headers, message) = split_headers("commit", payload)?;
        let mut tree = None;
        let mut parents = Vec::new();
        let mut author = None;
        let mut committer = None;
        let mut extra_headers = Vec::new();

        for (key, value) in headers {
            match key {
                b"tree" => tree = Some(hex_oid("commit", &value)?),
                b"parent" => parents.push(hex_oid("commit", &value)?),
                b"author" => author = Some(Signature::parse(&value)?),
                b"committer" => committer = Some(Signature::parse(&value)?),
                _ => extra_headers.push((header_name("commit", key)?, value)),
            }
        }

        Ok(Commit {
            tree: tree.ok_or_else(|| Error::corrupt("commit", "missing tree header"))?,
            parents,
            author: author.ok_or_else(|| Error::corrupt("commit", "missing author header"))?,
            committer: committer
                .ok_or_else(|| Error::corrupt("commit", "missing committer header"))?,
            message: message.to_vec(),
            extra_headers,
        })
    }

    pub fn summary(&self) -> String {
        let line = self
            .message
            .split(|b| *b == b'\n')
            .next()
            .unwrap_or_default();
        String::from_utf8_lossy(line).into_owned()
    }

    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

/// 头部块：`(key, value)` 对，按读入顺序。
pub(crate) type Headers<'a> = Vec<(&'a [u8], Vec<u8>)>;

/// 把对象载荷拆成「头部（按读入顺序）+ message」。
///
/// commit 与 tag 共用同一套规则：头部行形如 `key value`，值可以跨行
/// （续行以一个空格开头，该空格属于值本身）；第一处空行之后的所有字节
/// 都是 message，**原样保留、不做 trim** —— 逐字节往返依赖这一点。
pub(crate) fn split_headers<'a>(what: &str, payload: &'a [u8]) -> Result<(Headers<'a>, &'a [u8])> {
    let mut headers = Vec::new();
    let mut rest = payload;
    loop {
        if rest.is_empty() {
            return Err(Error::corrupt(
                what,
                "missing the blank line that separates headers from the message",
            ));
        }
        let (line, after) = split_line(rest);
        if line.is_empty() {
            return Ok((headers, after));
        }
        let space = line.iter().position(|b| *b == b' ').ok_or_else(|| {
            Error::corrupt(
                what,
                format!(
                    "header line {:?} has no value",
                    String::from_utf8_lossy(line)
                ),
            )
        })?;
        if space == 0 {
            return Err(Error::corrupt(
                what,
                "header line starts with a space (only continuation lines may)",
            ));
        }

        let mut value = line[space + 1..].to_vec();
        let mut tail = after;
        loop {
            let (continuation, next) = split_line(tail);
            if continuation.first() != Some(&b' ') {
                break;
            }
            value.push(b'\n');
            value.extend_from_slice(continuation);
            tail = next;
        }
        headers.push((&line[..space], value));
        rest = tail;
    }
}

/// 按第一个 `\n` 切一行；行尾换行符不算在行内。
fn split_line(input: &[u8]) -> (&[u8], &[u8]) {
    match input.iter().position(|b| *b == b'\n') {
        Some(idx) => (&input[..idx], &input[idx + 1..]),
        None => (input, &input[input.len()..]),
    }
}

/// 解析头部里的十六进制对象 id。
pub(crate) fn hex_oid(what: &str, value: &[u8]) -> Result<Oid> {
    let text =
        std::str::from_utf8(value).map_err(|_| Error::corrupt(what, "non-UTF8 object id"))?;
    Oid::from_hex(text).map_err(|err| Error::corrupt(what, format!("bad object id: {err}")))
}

/// 头部名字要求是 UTF-8（`extra_headers` 的 key 是 `String`）。
pub(crate) fn header_name(what: &str, key: &[u8]) -> Result<String> {
    String::from_utf8(key.to_vec()).map_err(|_| Error::corrupt(what, "non-UTF8 header name"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{hash, Kind};

    // 真值来自真实 git（git version 2.55.0）。生成命令：
    //
    //   TMP=$(mktemp -d); cd "$TMP"; git init -q .
    //   printf 'x\n' > x.txt; git add -A
    //   GIT_AUTHOR_DATE='1700000000 +0800' GIT_COMMITTER_DATE='1700000000 +0800' \
    //     git -c user.name='A U Thor' -c user.email='a@example.com' commit -q -m $'subject\n\nbody line\n'
    //   git rev-parse HEAD                                  # => COMMIT_OID
    //   git cat-file commit HEAD | xxd -p | tr -d '\n'      # => COMMIT_PAYLOAD

    const COMMIT_OID: &str = "a8207dce7b8e1b53f055c7c1b18b84b8a97fb0d3";
    const COMMIT_PAYLOAD: &[u8] = b"tree 0479003445f4e5a5ff25360c607ca79ffe4e4ea1\nauthor A U Thor <a@example.com> 1700000000 +0800\ncommitter A U Thor <a@example.com> 1700000000 +0800\n\nsubject\n\nbody line\n";

    /// 同一个仓库里的 `git merge --no-ff side`（两个 parent，首父在前）。
    const MERGE_OID: &str = "49f9b2680e4971af187277eadcbc1727536470f3";
    const MERGE_PAYLOAD: &[u8] = b"tree e2fbe588dbedf981a7447fc7c93e8f6ac356bb33\nparent 5eb9df18a129055cb61107ebe8f85cf30d692265\nparent 38e4ada91948b1e5074dc1609f82307a13235577\nauthor A U Thor <a@example.com> 1700000000 +0800\ncommitter A U Thor <a@example.com> 1700000000 +0800\n\nmerge side\n";

    /// 带 `encoding` 与多行 `gpgsig` 扩展头的提交。git 不接受手写的
    /// gpgsig 通过 `git commit`，所以真值走 `git hash-object -w -t commit`
    /// + `git cat-file commit`（原样回读，git fsck 无报错）：
    ///
    ///   git hash-object -w -t commit --stdin < payload
    ///   git cat-file commit 81c35bb456e952ad67754e4b0953cf76a7a50606 | xxd -p
    const EXTRA_OID: &str = "81c35bb456e952ad67754e4b0953cf76a7a50606";
    const EXTRA_PAYLOAD: &[u8] = b"tree 0479003445f4e5a5ff25360c607ca79ffe4e4ea1\nauthor A U Thor <a@example.com> 1700000000 +0800\ncommitter A U Thor <a@example.com> 1700000000 +0800\nencoding ISO-8859-1\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n iQIzBAABCAAdFiEEexample\n -----END PGP SIGNATURE-----\n\ncaf\xe9\n";

    /// message 末尾没有换行的提交（同样经 `git hash-object` 落库后回读）。
    const NO_TRAILING_PAYLOAD: &[u8] = b"tree 0479003445f4e5a5ff25360c607ca79ffe4e4ea1\nauthor A U Thor <a@example.com> 1700000000 +0800\ncommitter A U Thor <a@example.com> 1700000000 +0800\n\nno newline at end";

    const TREE_HEX: &str = "0479003445f4e5a5ff25360c607ca79ffe4e4ea1";

    fn signature() -> Signature {
        Signature::new("A U Thor", "a@example.com", 1_700_000_000, "+0800")
    }

    #[test]
    fn decodes_a_commit_real_git_wrote() {
        let commit = Commit::decode_payload(COMMIT_PAYLOAD).unwrap();
        assert_eq!(commit.tree.to_hex(), TREE_HEX);
        assert!(commit.parents.is_empty());
        assert!(!commit.is_merge());
        assert_eq!(commit.author, signature());
        assert_eq!(commit.committer, signature());
        assert_eq!(commit.message, b"subject\n\nbody line\n");
        assert_eq!(commit.summary(), "subject");
        assert!(commit.extra_headers.is_empty());

        // 编回来必须逐字节一致 —— 包括 message 末尾那个换行。
        assert_eq!(commit.encode_payload(), COMMIT_PAYLOAD);
        assert_eq!(
            hash(Kind::Commit, &commit.encode_payload()).to_hex(),
            COMMIT_OID
        );
    }

    #[test]
    fn keeps_parents_in_order() {
        let commit = Commit::decode_payload(MERGE_PAYLOAD).unwrap();
        assert!(commit.is_merge());
        assert_eq!(
            commit.parents.iter().map(Oid::to_hex).collect::<Vec<_>>(),
            vec![
                "5eb9df18a129055cb61107ebe8f85cf30d692265".to_string(),
                "38e4ada91948b1e5074dc1609f82307a13235577".to_string(),
            ]
        );
        assert_eq!(commit.encode_payload(), MERGE_PAYLOAD);
        assert_eq!(
            hash(Kind::Commit, &commit.encode_payload()).to_hex(),
            MERGE_OID
        );
    }

    #[test]
    fn preserves_multiline_extra_headers() {
        let commit = Commit::decode_payload(EXTRA_PAYLOAD).unwrap();
        assert_eq!(
            commit.extra_headers,
            vec![
                ("encoding".to_string(), b"ISO-8859-1".to_vec()),
                (
                    "gpgsig".to_string(),
                    b"-----BEGIN PGP SIGNATURE-----\n \n iQIzBAABCAAdFiEEexample\n -----END PGP SIGNATURE-----".to_vec(),
                ),
            ],
            "extra headers must keep their read order and their raw bytes"
        );
        assert_eq!(commit.message, b"caf\xe9\n");
        assert_eq!(commit.encode_payload(), EXTRA_PAYLOAD);
        assert_eq!(
            hash(Kind::Commit, &commit.encode_payload()).to_hex(),
            EXTRA_OID
        );
    }

    #[test]
    fn message_trailing_newline_is_not_invented() {
        let commit = Commit::decode_payload(NO_TRAILING_PAYLOAD).unwrap();
        assert_eq!(commit.message, b"no newline at end");
        assert_eq!(commit.encode_payload(), NO_TRAILING_PAYLOAD);
    }

    #[test]
    fn signature_roundtrips_and_defaults_to_empty_name() {
        let parsed = Signature::parse(b"A U Thor <a@example.com> 1700000000 +0800").unwrap();
        assert_eq!(parsed, signature());
        assert_eq!(
            parsed.format(),
            b"A U Thor <a@example.com> 1700000000 +0800".to_vec()
        );

        // 名字为空时 git 写出 `<space><email>`，解析结果必须能原样编回。
        let empty = Signature::parse(b" <a@example.com> 1700000000 +0000").unwrap();
        assert_eq!(empty.name, b"");
        assert_eq!(
            empty.format(),
            b" <a@example.com> 1700000000 +0000".to_vec()
        );

        // 负时区与老时间戳照样往返。
        let west = Signature::parse(b"Bob <b@x.y> 0 -0430").unwrap();
        assert_eq!((west.when, west.tz.as_str()), (0, "-0430"));
        assert_eq!(west.format(), b"Bob <b@x.y> 0 -0430".to_vec());
    }

    #[test]
    fn malformed_commits_are_rejected() {
        assert!(Signature::parse(b"no email here 1700000000 +0800").is_err());
        assert!(Signature::parse(b"A <a@b> not-a-timestamp +0800").is_err());
        assert!(Signature::parse(b"A <a@b> 1700000000").is_err());
        assert!(Signature::parse(b"A <a@b> 1700000000 +08").is_err());

        // 缺头部 / 缺头部与 message 之间的空行
        assert!(Commit::decode_payload(b"author A <a@b> 1 +0000\n\nx").is_err());
        assert!(Commit::decode_payload(
            b"tree 0479003445f4e5a5ff25360c607ca79ffe4e4ea1\nauthor A <a@b> 1 +0000\ncommitter A <a@b> 1 +0000\n"
        )
        .is_err());
    }
}
