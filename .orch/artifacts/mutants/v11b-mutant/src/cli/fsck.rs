//! `mg fsck` —— **T15（omp）**。
//!
//! 检查：所有 ref 指向的对象存在、所有可达对象 id 与内容一致、
//! tree/commit 头格式正确、index trailer 正确、pack 的对象数与 `.idx` 一致。
//! 验收：对真实 git 仓库跑 `mg fsck` 全绿；故意损坏一个对象后必须报错。

use crate::error::{todo, Result};

pub fn run(_full: bool) -> Result<()> {
    todo("cli::fsck (T15)")
}
