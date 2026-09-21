//! 行级 diff（Myers）与 unified 输出。
//!
//! **CONTROLLER-OWNED（`pub` 声明区冻结）** —— 属 **T6（omp）**。
//!
//! 验收：同一对 blob，`mg diff` 输出与 `git diff` 逐字节一致，
//! 包括 hunk 头的 `@@ -a,b +c,d @@`、上下文行数（默认 3）、
//! `\ No newline at end of file` 标记，以及 hunk 合并阈值（相邻 hunk 间距 ≤ 2×context 时合并）。

pub mod myers;
pub mod unified;

use crate::error::Result;

pub use myers::{myers, Edit, Op};
pub use unified::{hunks, unified, Hunk, HunkLine};

/// 按行切分，**保留行尾 `\n`**；末行没有 `\n` 时就是最后一段。
/// git 的 diff 完全建立在这个切分之上，先把它固定下来。
pub fn split_lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (idx, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            out.push(&bytes[start..=idx]);
            start = idx + 1;
        }
    }
    if start < bytes.len() {
        out.push(&bytes[start..]);
    }
    out
}

/// 行数（按 `split_lines` 的定义，末行无 `\n` 也算一行）。
pub fn line_count(bytes: &[u8]) -> usize {
    split_lines(bytes).len()
}

/// 生成 unified diff 文本；`a_label` / `b_label` 形如 `a/f.txt` / `b/f.txt`。
pub fn diff_texts(
    a: &[u8],
    b: &[u8],
    a_label: &str,
    b_label: &str,
    context: usize,
) -> Result<String> {
    unified::unified(a, b, a_label, b_label, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_keeping_newline() {
        let lines = split_lines(b"a\nb\n");
        assert_eq!(lines, vec![&b"a\n"[..], &b"b\n"[..]]);
    }

    #[test]
    fn splits_missing_final_newline() {
        let lines = split_lines(b"a\nb");
        assert_eq!(lines, vec![&b"a\n"[..], &b"b"[..]]);
        assert_eq!(line_count(b"a\nb"), 2);
    }

    #[test]
    fn empty_input_has_no_lines() {
        assert_eq!(split_lines(b""), Vec::<&[u8]>::new());
    }
}
