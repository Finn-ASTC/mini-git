//! annotated tag 对象。
//!
//! **T1 待实现范围**：`encode_payload` / `decode_payload`。
//! 格式：`object <oid>` / `type <kind>` / `tag <name>` / `tagger <signature>` / 空行 / message。
//! 验收：`mg cat-file -p <tag>` 与 `git cat-file -p <tag>` 一致。

use crate::error::{Error, Result};
use crate::object::commit::{header_name, hex_oid, split_headers};
use crate::object::{Kind, Signature};
use crate::oid::Oid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub object: Oid,
    pub kind: Kind,
    pub name: Vec<u8>,
    pub tagger: Option<Signature>,
    pub message: Vec<u8>,
    pub extra_headers: Vec<(String, Vec<u8>)>,
}

impl Tag {
    pub fn encode_payload(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"object ");
        out.extend_from_slice(self.object.to_hex().as_bytes());
        out.push(b'\n');
        out.extend_from_slice(b"type ");
        out.extend_from_slice(self.kind.as_str().as_bytes());
        out.push(b'\n');
        out.extend_from_slice(b"tag ");
        out.extend_from_slice(&self.name);
        out.push(b'\n');
        if let Some(tagger) = &self.tagger {
            out.extend_from_slice(b"tagger ");
            out.extend_from_slice(&tagger.format());
            out.push(b'\n');
        }
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
        let (headers, message) = split_headers("tag", payload)?;
        let mut object = None;
        let mut kind = None;
        let mut name = None;
        let mut tagger = None;
        let mut extra_headers = Vec::new();

        for (key, value) in headers {
            match key {
                b"object" => object = Some(hex_oid("tag", &value)?),
                b"type" => kind = Some(Kind::from_bytes(&value)?),
                b"tag" => name = Some(value),
                b"tagger" => tagger = Some(Signature::parse(&value)?),
                _ => extra_headers.push((header_name("tag", key)?, value)),
            }
        }

        Ok(Tag {
            object: object.ok_or_else(|| Error::corrupt("tag", "missing object header"))?,
            kind: kind.ok_or_else(|| Error::corrupt("tag", "missing type header"))?,
            name: name.ok_or_else(|| Error::corrupt("tag", "missing tag header"))?,
            tagger,
            message: message.to_vec(),
            extra_headers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::hash;

    // 真值来自真实 git（git version 2.55.0）。生成命令：
    //
    //   TMP=$(mktemp -d); cd "$TMP"; git init -q .
    //   printf 'x\n' > x.txt; git add -A
    //   GIT_AUTHOR_DATE='1700000000 +0800' GIT_COMMITTER_DATE='1700000000 +0800' \
    //     git -c user.name='A U Thor' -c user.email='a@example.com' commit -q -m $'subject\n\nbody line\n'
    //   GIT_COMMITTER_DATE='1700000000 +0800' \
    //     git -c user.name='A U Thor' -c user.email='a@example.com' tag -a v1 -m 'tag message' HEAD
    //   git rev-parse v1                                # => TAG_OID
    //   git cat-file tag v1 | xxd -p | tr -d '\n'       # => TAG_PAYLOAD

    const TAG_OID: &str = "18391d2eb484632c5316f0e943df3a0942498f12";
    const TAG_PAYLOAD: &[u8] = b"object a8207dce7b8e1b53f055c7c1b18b84b8a97fb0d3\ntype commit\ntag v1\ntagger A U Thor <a@example.com> 1700000000 +0800\n\ntag message\n";

    #[test]
    fn decodes_a_tag_real_git_wrote() {
        let tag = Tag::decode_payload(TAG_PAYLOAD).unwrap();
        assert_eq!(tag.object.to_hex(), "a8207dce7b8e1b53f055c7c1b18b84b8a97fb0d3");
        assert_eq!(tag.kind, Kind::Commit);
        assert_eq!(tag.name, b"v1");
        assert_eq!(
            tag.tagger,
            Some(Signature::new(
                "A U Thor",
                "a@example.com",
                1_700_000_000,
                "+0800"
            ))
        );
        assert_eq!(tag.message, b"tag message\n");
        assert!(tag.extra_headers.is_empty());

        assert_eq!(tag.encode_payload(), TAG_PAYLOAD);
        assert_eq!(
            hash(Kind::Tag, &tag.encode_payload()).to_hex(),
            TAG_OID
        );
    }

    #[test]
    fn a_missing_tagger_stays_missing() {
        // 无 tagger 的 tag 载荷（git 自己的 fsck 会拒收它，但对象格式允许）。
        let payload: &[u8] = b"object a8207dce7b8e1b53f055c7c1b18b84b8a97fb0d3\ntype commit\ntag bare\n\nmsg\n";
        let tag = Tag::decode_payload(payload).unwrap();
        assert_eq!(tag.tagger, None);
        assert_eq!(tag.name, b"bare");
        assert_eq!(tag.message, b"msg\n");
        assert_eq!(
            tag.encode_payload(),
            payload,
            "must not invent a tagger line"
        );
    }

    #[test]
    fn extra_headers_roundtrip() {
        let payload: &[u8] =
            b"object a8207dce7b8e1b53f055c7c1b18b84b8a97fb0d3\ntype commit\ntag v2\ntagger A <a@b> 1 +0000\nencoding UTF-8\n\nbody";
        let tag = Tag::decode_payload(payload).unwrap();
        assert_eq!(
            tag.extra_headers,
            vec![("encoding".to_string(), b"UTF-8".to_vec())]
        );
        assert_eq!(tag.message, b"body");
        assert_eq!(tag.encode_payload(), payload);
    }

    #[test]
    fn malformed_tags_are_rejected() {
        assert!(Tag::decode_payload(b"type commit\ntag v1\n\n").is_err());
        assert!(Tag::decode_payload(
            b"object a8207dce7b8e1b53f055c7c1b18b84b8a97fb0d3\ntag v1\n\n"
        )
        .is_err());
        assert!(Tag::decode_payload(
            b"object a8207dce7b8e1b53f055c7c1b18b84b8a97fb0d3\ntype blobfish\ntag v1\n\n"
        )
        .is_err());
    }
}
