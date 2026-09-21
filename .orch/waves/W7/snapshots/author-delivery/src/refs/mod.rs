//! 引用：HEAD、loose refs、packed-refs。
//!
//! **CONTROLLER-OWNED（`pub` 声明区冻结）** —— **T2（hermes）** 实现全部方法。
//!
//! 规则：
//! * `HEAD` 要么是 `ref: refs/heads/x\n`（attached），要么是裸 oid（detached）。
//! * loose ref（`.git/refs/...`）优先于 `.git/packed-refs` 中的同名条目。
//! * 更新必须原子：写 `*.lock` 或临时文件后 rename；带 `expected` 时做 compare-and-swap。
//!
//! 验收：`git symbolic-ref HEAD`、`git show-ref` 与 mg 一致；
//! CAS 在旧值不匹配时必须拒绝（`Error::RefConflict`）。

use crate::oid::Oid;
use crate::repo::Repo;

mod store;

pub const HEADS_PREFIX: &str = "refs/heads/";
pub const TAGS_PREFIX: &str = "refs/tags/";
pub const REMOTES_PREFIX: &str = "refs/remotes/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// 形如 `refs/heads/main`（已含 `refs/heads/` 前缀）。
    Attached(String),
    Detached(Oid),
}

impl Head {
    pub fn branch_name(&self) -> Option<&str> {
        match self {
            Head::Attached(name) => name.strip_prefix(HEADS_PREFIX),
            Head::Detached(_) => None,
        }
    }
}

pub struct RefStore<'a> {
    repo: &'a Repo,
}
