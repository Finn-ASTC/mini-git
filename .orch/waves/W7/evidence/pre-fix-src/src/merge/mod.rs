//! 合并：merge-base 与三方合并。
//!
//! **CONTROLLER-OWNED（`pub` 声明区冻结）**；
//! `merge_base.rs` 属 **T10 前置 / T7（codex）**，`three_way.rs` 属 **T10（opencode）**。

pub mod merge_base;
pub mod three_way;

use crate::error::Result;
use crate::oid::Oid;
use crate::repo::Repo;

pub use three_way::{MergeLabels, Merged};

/// 两个提交的最近公共祖先（LCA）。无共同祖先返回 `Ok(None)`。
pub fn merge_base(repo: &Repo, a: Oid, b: Oid) -> Result<Option<Oid>> {
    merge_base::merge_base(repo, a, b)
}
