//! git 对象模型：blob / tree / commit / tag。
//!
//! **CONTROLLER-OWNED（`pub` 声明区冻结）** —— 本文件由 controller 维护；
//! `blob.rs` / `tree.rs` / `commit.rs` / `tag.rs` 是实现文件，属于对应模块 agent。
//!
//! W0 已预置：`Kind`、`encode`、`decode`、`Object`、`FileMode`、`Blob`。
//! 待实现（T1）：`Tree` 的排序与编解码、`Commit`/`Tag`/`Signature`。

pub mod blob;
pub mod commit;
pub mod tag;
pub mod tree;

pub use blob::Blob;
pub use commit::{Commit, Signature};
pub use tag::Tag;
pub use tree::{Tree, TreeEntry};

use crate::error::{Error, Result};
use crate::oid::Oid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Blob,
    Tree,
    Commit,
    Tag,
}

impl Kind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Kind::Blob => "blob",
            Kind::Tree => "tree",
            Kind::Commit => "commit",
            Kind::Tag => "tag",
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        match bytes {
            b"blob" => Ok(Kind::Blob),
            b"tree" => Ok(Kind::Tree),
            b"commit" => Ok(Kind::Commit),
            b"tag" => Ok(Kind::Tag),
            other => Err(Error::corrupt(
                "object header",
                format!("unknown object type {:?}", String::from_utf8_lossy(other)),
            )),
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 生成 loose object 的完整字节：`"<type> <size>\0<payload>"`（未压缩）。
pub fn encode(kind: Kind, payload: &[u8]) -> Vec<u8> {
    let mut out = format!("{} {}\0", kind.as_str(), payload.len()).into_bytes();
    out.extend_from_slice(payload);
    out
}

/// 解析 `"<type> <size>\0<payload>"`，返回类型与 payload。
pub fn decode(bytes: &[u8]) -> Result<(Kind, Vec<u8>)> {
    let nul = bytes
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| Error::corrupt("object header", "missing NUL terminator"))?;
    let header = &bytes[..nul];
    let space = header
        .iter()
        .position(|b| *b == b' ')
        .ok_or_else(|| Error::corrupt("object header", "missing type/size separator"))?;
    let kind = Kind::from_bytes(&header[..space])?;
    let size_text = std::str::from_utf8(&header[space + 1..])
        .map_err(|_| Error::corrupt("object header", "non-UTF8 size field"))?;
    let size: usize = size_text
        .trim()
        .parse()
        .map_err(|_| Error::corrupt("object header", format!("bad size {size_text:?}")))?;
    let payload = &bytes[nul + 1..];
    if payload.len() != size {
        return Err(Error::corrupt(
            "object",
            format!("header says {size} bytes, payload is {}", payload.len()),
        ));
    }
    Ok((kind, payload.to_vec()))
}

/// 对象 id = `sha1(encode(kind, payload))`。
pub fn hash(kind: Kind, payload: &[u8]) -> Oid {
    Oid::hash_object(kind.as_str(), payload)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Object {
    Blob(Blob),
    Tree(Tree),
    Commit(Commit),
    Tag(Tag),
}

impl Object {
    pub fn kind(&self) -> Kind {
        match self {
            Object::Blob(_) => Kind::Blob,
            Object::Tree(_) => Kind::Tree,
            Object::Commit(_) => Kind::Commit,
            Object::Tag(_) => Kind::Tag,
        }
    }

    pub fn encode_payload(&self) -> Vec<u8> {
        match self {
            Object::Blob(blob) => blob.encode_payload(),
            Object::Tree(tree) => tree.encode_payload(),
            Object::Commit(commit) => commit.encode_payload(),
            Object::Tag(tag) => tag.encode_payload(),
        }
    }

    pub fn decode(kind: Kind, payload: &[u8]) -> Result<Object> {
        Ok(match kind {
            Kind::Blob => Object::Blob(Blob::decode_payload(payload)?),
            Kind::Tree => Object::Tree(Tree::decode_payload(payload)?),
            Kind::Commit => Object::Commit(Commit::decode_payload(payload)?),
            Kind::Tag => Object::Tag(Tag::decode_payload(payload)?),
        })
    }

    /// 便捷：返回 commit，否则报类型错误。
    pub fn into_commit(self) -> Result<Commit> {
        match self {
            Object::Commit(commit) => Ok(commit),
            other => Err(Error::Other(format!(
                "expected commit, got {}",
                other.kind()
            ))),
        }
    }

    pub fn into_tree(self) -> Result<Tree> {
        match self {
            Object::Tree(tree) => Ok(tree),
            other => Err(Error::Other(format!("expected tree, got {}", other.kind()))),
        }
    }
}

/// 文件模式。树条目与 index 都使用它；`40000` 必须写成 5 位（不是 `040000`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileMode {
    Regular,
    Executable,
    Symlink,
    Tree,
}

impl FileMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            FileMode::Regular => "100644",
            FileMode::Executable => "100755",
            FileMode::Symlink => "120000",
            FileMode::Tree => "40000",
        }
    }

    pub const fn to_u32(self) -> u32 {
        match self {
            FileMode::Regular => 0o100644,
            FileMode::Executable => 0o100755,
            FileMode::Symlink => 0o120000,
            FileMode::Tree => 0o040000,
        }
    }

    pub const fn is_tree(self) -> bool {
        matches!(self, FileMode::Tree)
    }

    pub const fn is_symlink(self) -> bool {
        matches!(self, FileMode::Symlink)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        match bytes {
            b"100644" => Ok(FileMode::Regular),
            b"100755" => Ok(FileMode::Executable),
            b"120000" => Ok(FileMode::Symlink),
            // git 规范写法是 "40000"，但容忍 6 位写法。
            b"40000" | b"040000" => Ok(FileMode::Tree),
            // gitlink（submodule）在 v1 明确不支持；必须是 Unsupported 而不是 Corrupt，
            // 否则调用方会误判为「文件损坏」。（controller，C-13）
            b"160000" => Err(Error::Unsupported(
                "gitlink (submodule) entries are not supported in v1",
            )),
            other => Err(Error::corrupt(
                "tree entry",
                format!("unknown file mode {:?}", String::from_utf8_lossy(other)),
            )),
        }
    }

    pub fn from_u32(mode: u32) -> Result<Self> {
        match mode {
            0o100644 => Ok(FileMode::Regular),
            0o100755 => Ok(FileMode::Executable),
            0o120000 => Ok(FileMode::Symlink),
            0o040000 => Ok(FileMode::Tree),
            // 同上：gitlink 是「不支持」而不是「损坏」（controller，C-13）。
            0o160000 => Err(Error::Unsupported(
                "gitlink (submodule) entries are not supported in v1",
            )),
            other => Err(Error::corrupt(
                "index entry",
                format!("unsupported file mode {other:o}"),
            )),
        }
    }
}

impl std::fmt::Display for FileMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let raw = encode(Kind::Blob, b"hello world\n");
        assert_eq!(raw, b"blob 12\0hello world\n");
        let (kind, payload) = decode(&raw).unwrap();
        assert_eq!(kind, Kind::Blob);
        assert_eq!(payload, b"hello world\n");
        assert_eq!(
            hash(Kind::Blob, b"hello world\n").to_hex(),
            "3b18e512dba79e4c8300dd08aeb37f8e728b8dad"
        );
    }

    #[test]
    fn size_mismatch_is_corrupt() {
        assert!(decode(b"blob 99\0short").is_err());
        assert!(decode(b"blob 3").is_err());
    }

    #[test]
    fn file_mode_strings() {
        assert_eq!(FileMode::Tree.as_str(), "40000");
        assert_eq!(FileMode::from_bytes(b"40000").unwrap(), FileMode::Tree);
        assert!(FileMode::from_bytes(b"100664").is_err());
    }
}
