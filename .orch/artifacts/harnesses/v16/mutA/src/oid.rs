//! 对象 id（SHA-1，20 字节）。CONTROLLER-OWNED —— 冻结，实现已完成。
//!
//! 对象 id 的计算规则：对 **未压缩** 的 `"<type> <size>\0<payload>"` 取 SHA-1。
//! 注意这里是「含 header」的字节；`hash_object()` 是唯一允许做这件事的地方。

use std::fmt;
use std::str::FromStr;

use sha1::{Digest, Sha1};

use crate::error::{Error, Result};

pub const RAW_LEN: usize = 20;
pub const HEX_LEN: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid([u8; RAW_LEN]);

impl Oid {
    pub const fn from_bytes(bytes: [u8; RAW_LEN]) -> Self {
        Oid(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != RAW_LEN {
            return Err(Error::InvalidOid(format!(
                "expected {RAW_LEN} raw bytes, got {}",
                bytes.len()
            )));
        }
        let mut raw = [0u8; RAW_LEN];
        raw.copy_from_slice(bytes);
        Ok(Oid(raw))
    }

    pub fn from_hex(hex: &str) -> Result<Self> {
        let hex = hex.trim();
        if hex.len() != HEX_LEN {
            return Err(Error::InvalidOid(format!(
                "expected {HEX_LEN} hex digits, got {} ({hex:?})",
                hex.len()
            )));
        }
        let mut raw = [0u8; RAW_LEN];
        for (i, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
            let hi = hex_val(chunk[0])?;
            let lo = hex_val(chunk[1])?;
            raw[i] = (hi << 4) | lo;
        }
        Ok(Oid(raw))
    }

    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(HEX_LEN);
        for byte in self.0 {
            out.push(HEX_CHARS[(byte >> 4) as usize] as char);
            out.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
        }
        out
    }

    pub fn as_bytes(&self) -> &[u8; RAW_LEN] {
        &self.0
    }

    pub const fn zeros() -> Self {
        Oid([0u8; RAW_LEN])
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; RAW_LEN]
    }

    /// 唯一计算对象 id 的入口：`sha1("<type> <size>\0<payload>")`。
    pub fn hash_object(kind: &str, payload: &[u8]) -> Oid {
        let header = format!("{kind} {}\0", payload.len());
        let mut hasher = Sha1::new();
        hasher.update(header.as_bytes());
        hasher.update(payload);
        let digest = hasher.finalize();
        let mut raw = [0u8; RAW_LEN];
        raw.copy_from_slice(&digest[..RAW_LEN]);
        Oid(raw)
    }

    /// `ab/cdef..` 形式的相对路径（`.git/objects/` 之下）。
    pub fn loose_rel_path(&self) -> String {
        let hex = self.to_hex();
        format!("{}/{}", &hex[..2], &hex[2..])
    }
}

const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

fn hex_val(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::InvalidOid(format!(
            "non-hex character {:?}",
            byte as char
        ))),
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.to_hex())
    }
}

impl FromStr for Oid {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Oid::from_hex(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let hex = "3b18e512dba79e4c8300dd08aeb37f8e728b8dad";
        let oid = Oid::from_hex(hex).unwrap();
        assert_eq!(oid.to_hex(), hex);
        assert_eq!(
            oid.loose_rel_path(),
            "3b/18e512dba79e4c8300dd08aeb37f8e728b8dad"
        );
    }

    #[test]
    fn known_blob_hash() {
        // `printf 'hello world\n' | git hash-object --stdin`
        let oid = Oid::hash_object("blob", b"hello world\n");
        assert_eq!(oid.to_hex(), "3b18e512dba79e4c8300dd08aeb37f8e728b8dad");
    }

    #[test]
    fn rejects_bad_hex() {
        assert!(Oid::from_hex("zz").is_err());
        assert!(Oid::from_hex("").is_err());
    }
}
