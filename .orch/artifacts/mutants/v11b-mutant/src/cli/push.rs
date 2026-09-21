//! `mg push` —— **T13（codex）/ T14（opencode）**。
//!
//! 必须拒绝 non-fast-forward（除非 `--force`），并解析 `report-status` 的错误原因。
//! 验收：`git clone` 出来的仓库能看到 mg push 的提交；non-FF 时退出码非 0 且远端未被改动。

use crate::error::{todo, Result};

pub fn run(
    _remote: Option<&str>,
    _refspec: Option<&str>,
    _set_upstream: bool,
    _force: bool,
) -> Result<()> {
    todo("cli::push (T13/T14)")
}
