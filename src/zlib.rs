//! zlib 压缩/解压封装。CONTROLLER-OWNED —— 冻结，实现已完成。
//!
//! git 的 loose object 与 pack 中的对象数据都是 zlib stream。
//! 压缩级别不影响对象 id（id 只取决于未压缩字节），但固定级别可让测试更稳定。

use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::error::{Error, Result};

/// git 默认 `core.compression` 是 1（最快），这里固定用 6 换取体积；两者都能被 git 解压。
pub const DEFAULT_LEVEL: u32 = 6;

pub fn inflate_all(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(bytes);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|err| Error::corrupt("zlib stream", err.to_string()))?;
    Ok(out)
}

pub fn deflate(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(DEFAULT_LEVEL));
    encoder.write_all(bytes)?;
    encoder
        .finish()
        .map_err(|err| Error::corrupt("zlib stream", err.to_string()))
}

/// 从 zlib stream 头部解出「解压后的大小」是不必要的——git 的 header 里已写明。
/// 这个辅助函数用于 pack 解析：解压前若干字节并返回剩余输入。
pub fn inflate_prefix(bytes: &[u8]) -> Result<(Vec<u8>, usize)> {
    let mut decoder = ZlibDecoder::new(bytes);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|err| Error::corrupt("zlib stream", err.to_string()))?;
    let consumed = decoder.total_in() as usize;
    Ok((out, consumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let payload = b"blob 12\0hello world\n".to_vec();
        let packed = deflate(&payload).unwrap();
        assert_eq!(inflate_all(&packed).unwrap(), payload);
    }

    #[test]
    fn prefix_reports_consumed_bytes() {
        let a = deflate(b"first").unwrap();
        let mut joined = a.clone();
        joined.extend_from_slice(b"TRAILING");
        let (out, consumed) = inflate_prefix(&joined).unwrap();
        assert_eq!(out, b"first");
        assert_eq!(consumed, a.len());
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(inflate_all(b"not zlib at all").is_err());
    }
}
