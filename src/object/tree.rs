//! tree 对象。
//!
//! **T1 待实现范围**：条目排序规则、`encode_payload` / `decode_payload`。
//!
//! 排序陷阱：按 name 字节序，但**子树按其名字末尾多一个 `/` 参与比较**
//! （`/`=0x2F > `.`=0x2E，所以 `foo.txt` 排在 `foo/` 之前）。
//! 编码格式：`"<mode> <name>\0"` + 20 字节二进制 oid，重复拼接。
//!
//! 验收：`mg write-tree` 出来的 oid 与 `git write-tree` 相同。

use std::cmp::Ordering;

use crate::error::{Error, Result};
use crate::object::FileMode;
use crate::oid::{Oid, RAW_LEN};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: FileMode,
    pub name: Vec<u8>,
    pub oid: Oid,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree(pub Vec<TreeEntry>);

impl Tree {
    pub fn new(entries: Vec<TreeEntry>) -> Self {
        Tree(entries)
    }

    pub fn entries(&self) -> &[TreeEntry] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 按 git 规则就地对条目排序（见文件头注释）。
    pub fn sort_entries(entries: &mut [TreeEntry]) -> Result<()> {
        entries.sort_by(compare_entries);
        Ok(())
    }

    pub fn encode_payload(&self) -> Vec<u8> {
        // 排序是编码的一部分：git 的 tree 载荷必须是规范序，否则 oid 就不对。
        let mut entries = self.0.clone();
        entries.sort_by(compare_entries);

        let mut out = Vec::with_capacity(
            entries
                .iter()
                .map(|entry| entry.mode.as_str().len() + 1 + entry.name.len() + 1 + RAW_LEN)
                .sum(),
        );
        for entry in &entries {
            out.extend_from_slice(entry.mode.as_str().as_bytes());
            out.push(b' ');
            out.extend_from_slice(&entry.name);
            out.push(0);
            out.extend_from_slice(entry.oid.as_bytes());
        }
        out
    }

    pub fn decode_payload(payload: &[u8]) -> Result<Self> {
        let mut entries = Vec::new();
        let mut rest = payload;
        while !rest.is_empty() {
            let nul = rest
                .iter()
                .position(|b| *b == 0)
                .ok_or_else(|| Error::corrupt("tree", "entry is missing its NUL terminator"))?;
            let header = &rest[..nul];
            let space = header.iter().position(|b| *b == b' ').ok_or_else(|| {
                Error::corrupt("tree", "entry is missing the mode/name separator")
            })?;
            let mode = FileMode::from_bytes(&header[..space])?;
            let name = header[space + 1..].to_vec();

            let oid_start = nul + 1;
            let oid_end = oid_start + RAW_LEN;
            let raw = rest.get(oid_start..oid_end).ok_or_else(|| {
                Error::corrupt(
                    "tree",
                    format!(
                        "entry {:?} is truncated (needs {RAW_LEN} bytes of object id)",
                        String::from_utf8_lossy(&name)
                    ),
                )
            })?;
            entries.push(TreeEntry {
                mode,
                name,
                oid: Oid::from_slice(raw)?,
            });
            rest = &rest[oid_end..];
        }
        Ok(Tree(entries))
    }

    /// 按 name 精确查找（不做路径拆分，只比这一层）。
    pub fn lookup(&self, name: &[u8]) -> Option<&TreeEntry> {
        self.0.iter().find(|entry| entry.name.as_slice() == name)
    }
}

/// git 的 `base_name_compare`：先按公共前缀比较，若一个是另一个的前缀，
/// 则短名字的「下一字节」在子树时视为 `/`（0x2F），否则视为字符串结束（0）。
fn compare_entries(a: &TreeEntry, b: &TreeEntry) -> Ordering {
    let shared = a.name.len().min(b.name.len());
    let prefix = a.name[..shared].cmp(&b.name[..shared]);
    if prefix != Ordering::Equal {
        return prefix;
    }
    let a_next = a
        .name
        .get(shared)
        .copied()
        .unwrap_or(if a.mode.is_tree() { b'/' } else { 0 });
    let b_next = b
        .name
        .get(shared)
        .copied()
        .unwrap_or(if b.mode.is_tree() { b'/' } else { 0 });
    a_next.cmp(&b_next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{hash, Kind};

    // 真值全部来自真实 git（git version 2.55.0）。生成命令：
    //
    //   TMP=$(mktemp -d); cd "$TMP"; git init -q .
    //   printf 'a\n' > a.txt; mkdir foo; printf 'b\n' > foo/b.txt; printf 'c\n' > foo.txt
    //   printf '#!/bin/sh\n' > run.sh; chmod +x run.sh; ln -s a.txt link.txt
    //   git add -A
    //   git write-tree                                  # => TREE_OID
    //   git cat-file tree "$(git write-tree)" | xxd -p | tr -d '\n'   # => TREE_PAYLOAD
    //   git cat-file tree "$(git write-tree):foo" | xxd -p | tr -d '\n'

    /// `git write-tree`
    const TREE_OID: &str = "4aad58e0401292349b1a142aa10cbcc061c2eda2";
    /// `git cat-file tree <TREE_OID>`（168 字节）
    const TREE_PAYLOAD: &[u8] = b"100644 a.txt\x00x\x98\x19\"a;*\xfb`%\x04/\xf6\xbd\x87\x8a\xc1\x99N\x85100644 foo.txt\x00\xf2\xadlv\xf0\x11Zk\xa5\xb0\x04V\xa8I\x81\x0e~\xc0\xaf 40000 foo\x00\xf8\xf7\xae\xfc)\x00\xa3\xd77\xce\xa9\xee\xe4W)\xfdUv\x1e\x1a120000 link.txt\x00\x8d\x14\xcb\xf9\x83\xb3\xfa\xd6\x83\x17\x1c\x94\x18\x99\x8d\x9fh4\x08#100755 run.sh\x00\x1a$\x85%\x1c3\xa7\x0429L\x93\xfb\x893\x0e\xf2\x14\xbf\xc9";

    /// `git cat-file tree <TREE_OID>:foo`
    const FOO_SUBTREE_PAYLOAD: &[u8] =
        b"100644 b.txt\x00ax\x07\x98\"\x8d\x17\xaf-4\xfc\xe4\xcf\xbd\xf3UV\x83$r";

    const A_TXT: &str = "78981922613b2afb6025042ff6bd878ac1994e85";
    const FOO_TXT: &str = "f2ad6c76f0115a6ba5b00456a849810e7ec0af20";
    const FOO_TREE: &str = "f8f7aefc2900a3d737cea9eee45729fd55761e1a";
    const LINK_TXT: &str = "8d14cbf983b3fad683171c9418998d9f68340823";
    const RUN_SH: &str = "1a2485251c33a70432394c93fb89330ef214bfc9";

    fn oid(hex: &str) -> Oid {
        Oid::from_hex(hex).unwrap()
    }

    fn entry(mode: FileMode, name: &[u8], hex: &str) -> TreeEntry {
        TreeEntry {
            mode,
            name: name.to_vec(),
            oid: oid(hex),
        }
    }

    /// `git ls-tree` 的规范顺序，也是 `TREE_PAYLOAD` 里的顺序。
    fn golden_tree() -> Tree {
        Tree::new(vec![
            entry(FileMode::Regular, b"a.txt", A_TXT),
            entry(FileMode::Regular, b"foo.txt", FOO_TXT),
            entry(FileMode::Tree, b"foo", FOO_TREE),
            entry(FileMode::Symlink, b"link.txt", LINK_TXT),
            entry(FileMode::Executable, b"run.sh", RUN_SH),
        ])
    }

    #[test]
    fn encodes_a_tree_real_git_wrote() {
        let tree = golden_tree();
        assert_eq!(tree.encode_payload(), TREE_PAYLOAD);
        assert_eq!(
            hash(Kind::Tree, &tree.encode_payload()).to_hex(),
            TREE_OID,
            "tree payload must hash to `git write-tree`'s oid"
        );
        // 子树（foo/）的载荷同样逐字节一致。
        let sub = Tree::new(vec![entry(
            FileMode::Regular,
            b"b.txt",
            "61780798228d17af2d34fce4cfbdf35556832472",
        )]);
        assert_eq!(sub.encode_payload(), FOO_SUBTREE_PAYLOAD);
    }

    #[test]
    fn decoding_git_bytes_roundtrips_byte_for_byte() {
        let tree = Tree::decode_payload(TREE_PAYLOAD).unwrap();
        assert_eq!(tree, golden_tree());
        assert_eq!(tree.encode_payload(), TREE_PAYLOAD);

        // 名字与 oid 都解析正确；`foo.txt` 与 `foo/` 的次序没被打乱。
        let names: Vec<&[u8]> = tree.entries().iter().map(|e| e.name.as_slice()).collect();
        assert_eq!(
            names,
            vec![
                b"a.txt".as_slice(),
                b"foo.txt".as_slice(),
                b"foo".as_slice(),
                b"link.txt".as_slice(),
                b"run.sh".as_slice(),
            ]
        );
        assert_eq!(tree.lookup(b"foo").unwrap().mode, FileMode::Tree);
        assert_eq!(tree.lookup(b"run.sh").unwrap().mode, FileMode::Executable);
        assert_eq!(tree.lookup(b"link.txt").unwrap().mode, FileMode::Symlink);
        assert_eq!(tree.lookup(b"link.txt").unwrap().oid.to_hex(), LINK_TXT);
        assert_eq!(tree.lookup(b"nope.txt"), None);
    }

    #[test]
    fn sorting_puts_foo_txt_before_the_foo_subtree() {
        // 故意乱序：子树在最前，且 `foo` 与 `foo.txt` 的陷阱项相邻。
        let mut entries = vec![
            entry(FileMode::Tree, b"foo", FOO_TREE),
            entry(FileMode::Executable, b"run.sh", RUN_SH),
            entry(FileMode::Regular, b"foo.txt", FOO_TXT),
            entry(FileMode::Symlink, b"link.txt", LINK_TXT),
            entry(FileMode::Regular, b"a.txt", A_TXT),
        ];
        Tree::sort_entries(&mut entries).unwrap();

        let names: Vec<&[u8]> = entries.iter().map(|e| e.name.as_slice()).collect();
        assert_eq!(
            names,
            vec![
                b"a.txt".as_slice(),
                b"foo.txt".as_slice(),
                b"foo".as_slice(),
                b"link.txt".as_slice(),
                b"run.sh".as_slice(),
            ]
        );
        // 乱序输入的 Tree 必须编码成 git 的规范字节 / oid。
        let tree = Tree::new(entries);
        assert_eq!(tree.encode_payload(), TREE_PAYLOAD);
        assert_eq!(hash(Kind::Tree, &tree.encode_payload()).to_hex(), TREE_OID);
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        // 缺 NUL 终止符
        assert!(Tree::decode_payload(b"100644 a.txt").is_err());
        // 缺 mode/name 分隔符
        assert!(Tree::decode_payload(b"100644\x00aaaaaaaaaaaaaaaaaaaa").is_err());
        // oid 被截断
        assert!(Tree::decode_payload(b"100644 a.txt\x00short").is_err());
        // 未知 mode
        assert!(Tree::decode_payload(b"100664 a.txt\x00aaaaaaaaaaaaaaaaaaaa").is_err());
        // 空载荷是合法空树（git 的 `git mktree </dev/null`）
        assert!(Tree::decode_payload(b"").unwrap().is_empty());
    }
}
