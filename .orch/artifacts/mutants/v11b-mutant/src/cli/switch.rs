//! `mg switch` —— **T11（omp）**。
//!
//! * `mg switch <branch>`：切到已有本地分支（`refs/heads/<branch>` 必须存在，
//!   否则 `ReferenceNotFound`，不会去 detach 到一个同名 tag/sha —— git 的 `switch` 同理）。
//! * `mg switch -c <new> <start>`：在 `<start>` 上建 `<new>` 并切过去。
//!   参考实现里 `-c` 后必须显式给起点：`Command::Switch` 的 positional 是必填的
//!   （`cli/mod.rs` CONTROLLER-OWNED，T11 无权改成 `Option`）。
//! * 安全：会被覆盖的本地改动 → `Error::WouldLoseChanges`；被拒绝时**不改变 HEAD、
//!   不留下新分支、不动工作区**（校验先于一切写操作）。
//! * 已经在目标分支上且没有 `-f` → 直接返回（git: `Already on '<branch>'`）。
//! * **目标提交 == 当前 HEAD 提交**且没有 `-f` 时，git 只改 HEAD、根本不碰 index 与工作区
//!   （暂存改动、本地删除、未跟踪文件都原样留下，实测 P1/P3/P8/P9/P10）——
//!   这条捷径由 `materialize::same_commit_switch` 判定，`switch -c` 的预校验也会跳过。
//! * `.git/MERGE_HEAD` 存在（合并进行中）→ 拒绝（git: `cannot switch branch while merging`）。

use crate::cli::open_repo;
use crate::error::{Error, Result};
use crate::index::Index;
use crate::refs::RefStore;
use crate::repo::Repo;
use crate::worktree::materialize::{
    branch_oid, check_tree, checkout_to, commit_tree, current_branch, full_ref, merge_in_progress,
    resolve_commit, same_commit_switch, tree_entries, HeadTarget,
};

use super::branch::validate_branch_name;

pub fn run(create: Option<&str>, name: &str, force: bool) -> Result<()> {
    let repo = open_repo()?;
    let refs = RefStore::new(&repo);
    if merge_in_progress(&repo) {
        // git: `fatal: cannot switch branch while merging`
        return Err(Error::Other(
            "cannot switch branch while merging".to_string(),
        ));
    }
    match create {
        Some(branch) => create_and_switch(&repo, &refs, branch, name, force),
        None => switch_existing(&repo, name, force),
    }
}

fn create_and_switch(
    repo: &Repo,
    refs: &RefStore<'_>,
    branch: &str,
    start: &str,
    force: bool,
) -> Result<()> {
    validate_branch_name(branch)?;
    let full = full_ref(branch);
    if refs.exists(&full) {
        return Err(Error::Other(format!(
            "a branch named '{branch}' already exists"
        )));
    }
    let start_oid = resolve_commit(repo, start)?;
    // 先校验再落引用：被拒绝时 git 也不会留下新分支（实测 E26）；
    // 起点就是当前提交时 git 只改 HEAD、不动物化（实测 P3），预校验同样跳过。
    if !same_commit_switch(repo, start_oid, force, &Index::read(repo)?)? {
        check_tree(repo, &tree_entries(repo, commit_tree(repo, start_oid)?)?, force)?;
    }
    refs.update(&full, start_oid, None)?;
    checkout_to(repo, start_oid, force, HeadTarget::Attached(branch))?;
    println!("Switched to a new branch '{branch}'");
    Ok(())
}

fn switch_existing(repo: &Repo, name: &str, force: bool) -> Result<()> {
    let Some(commit) = branch_oid(repo, name)? else {
        // `switch` 只切本地分支：不是分支就不是「已有的分支」。
        return Err(Error::RefNotFound(name.to_string()));
    };
    if !force && current_branch(repo)?.as_deref() == Some(name) {
        println!("Already on '{name}'");
        return Ok(());
    }
    checkout_to(repo, commit, force, HeadTarget::Attached(name))?;
    println!("Switched to branch '{name}'");
    Ok(())
}
