//! `mg clone` —— **T13（codex，file://）/ T14（opencode，http://）**。
//!
//! 验收：`mg clone file:///tmp/src dst` 后，dst 里 `git fsck` 通过、
//! `git log --oneline` 与源仓库一致、`git status` 干净。

use std::path::Path;

use crate::error::{todo, Result};

pub fn run(_url: &str, _dir: Option<&Path>) -> Result<()> {
    todo("cli::clone (T13/T14)")
}
