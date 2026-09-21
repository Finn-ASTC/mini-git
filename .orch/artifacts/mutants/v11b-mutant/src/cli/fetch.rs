//! `mg fetch` —— **T13（codex）/ T14（opencode）**。
//!
//! 更新的引用写到 `refs/remotes/<remote>/...`，并把 `FETCH_HEAD` 写好。
//! 验收：fetch 后 `git show-ref` 的远端引用存在且 oid 正确。

use crate::error::{todo, Result};

pub fn run(_remote: Option<&str>, _refspec: Option<&str>) -> Result<()> {
    todo("cli::fetch (T13/T14)")
}
