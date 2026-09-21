//! `file://` 传输（本地路径直读对端仓库）。**T13（codex）实现范围**。
//!
//! 实现方式：直接把对端仓库当作一个 `Repo` 打开，复用本地对象读取与 refs，
//! 但**必须走 pkt-line 编解码路径**（而不是直接调用本地 API），
//! 这样 T13 的产出的协商逻辑才能在 T14（HTTP）里原样复用。

use std::path::Path;

use crate::error::{todo, Result};
use crate::oid::Oid;
use crate::repo::Repo;

#[derive(Debug, Clone, Default)]
pub struct FetchOutcome {
    /// 更新后的远端引用（目标仓库中的名字，如 `refs/remotes/origin/main`）。
    pub refs: Vec<(String, Oid)>,
    pub head: Option<(String, Oid)>,
    pub objects_written: usize,
}

pub fn fetch(_repo: &Repo, _url: &str, _refspecs: &[String]) -> Result<FetchOutcome> {
    todo("transport::local::fetch (T13)")
}

pub fn push(_repo: &Repo, _url: &str, _refspecs: &[String], _force: bool) -> Result<()> {
    todo("transport::local::push (T13)")
}

/// `mg clone <url> <dir>`：建新仓库 → fetch → 检出 HEAD。
pub fn clone_into(_url: &str, _dir: &Path) -> Result<Repo> {
    todo("transport::local::clone_into (T13)")
}
