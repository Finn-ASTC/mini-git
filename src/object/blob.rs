//! blob 对象：裸文件内容。W0 已实现。

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob(pub Vec<u8>);

impl Blob {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Blob(bytes.into())
    }

    pub fn encode_payload(&self) -> Vec<u8> {
        self.0.clone()
    }

    pub fn decode_payload(payload: &[u8]) -> Result<Self> {
        Ok(Blob(payload.to_vec()))
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 是否为「看起来像文本」的内容（用于 diff 时判断要不要输出 "Binary files differ"）。
    pub fn looks_binary(&self) -> bool {
        self.0.iter().take(8000).any(|b| *b == 0)
    }
}
