//! `mg pull` —— **T13/T14 + T10**：fetch 后做 merge（fast-forward 优先）。
//!
//! fast-forward 路径在本文件内独立实现（用 T11 的 `apply_tree` / `tree_index`），
//! 非 fast-forward 才交给 `cli::merge` 的三方合并。
//!
//! # 为什么不直接 `cli::merge::run(dest)`
//!
//! 实测（git 2.55.0 仓库 + 子目录 `sub/b.txt`）：`cli/merge.rs` 的 fast-forward 分支
//! 把 **已展平** 的 tree（`flat_tree(flat)`）传给了 `worktree.materialize_tree`，
//! 而后者会把入参当**嵌套** tree 再展平一遍 → `Error::corrupt("tree entry name",
//! "sub/b.txt")`（`src/worktree/materialize.rs:124` 的 `validate_name`）。
//! 这是 T10 文件里的缺陷，而本任务的写作用域不含 `src/cli/merge.rs`，所以这里不修它，
//! 只在 pull 里绕开：FF 用 T11 的展平 API（`checkout_to` 走的同一条路），
//! 行为与真实 git 一致（已实测：`mg pull` 后 `git log`/`git status` 与 `git pull` 相同）。
//! **非 fast-forward 的三方合并仍会走进那个缺陷**（子目录仓库）。

use std::collections::BTreeSet;

use crate::cli::fetch::remote_and_url;
use crate::cli::open_repo;
use crate::error::{Error, Result};
use crate::index::Index;
use crate::oid::Oid;
use crate::refs::{Head, RefStore, HEADS_PREFIX, REMOTES_PREFIX};
use crate::repo::Repo;
use crate::transport::local;
use crate::worktree::materialize::{
    apply_tree, commit_tree, full_ref, tree_entries, tree_index,
};

pub fn run(remote: Option<&str>) -> Result<()> {
    let repo = open_repo()?;
    let (name, url) = remote_and_url(&repo, remote)?;
    let branch = match RefStore::new(&repo).read_head()? {
        Head::Attached(target) => target
            .strip_prefix(HEADS_PREFIX)
            .unwrap_or(target.as_str())
            .to_string(),
        Head::Detached(_) => {
            return Err(Error::Other(
                "You are not currently on a branch.".to_string(),
            ))
        }
    };
    let merge_ref = repo
        .config()
        .get(&format!("branch.{branch}.merge"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{HEADS_PREFIX}{branch}"));
    let upstream = merge_ref
        .strip_prefix(HEADS_PREFIX)
        .unwrap_or(merge_ref.as_str())
        .to_string();
    let dest = format!("{REMOTES_PREFIX}{name}/{upstream}");

    println!("From {url}");
    local::fetch(
        &repo,
        &url,
        &[format!("+{HEADS_PREFIX}{upstream}:{dest}")],
    )?;

    let refs = RefStore::new(&repo);
    let head = refs.resolve("HEAD")?;
    let target = refs.resolve(&dest)?;
    if target == head {
        println!("Already up to date.");
        return Ok(());
    }
    match crate::merge::merge_base(&repo, head, target)? {
        Some(base) if base == head => fast_forward(&repo, &branch, head, target),
        Some(_) => crate::cli::merge::run(&dest, None, false),
        None => Err(Error::Unsupported(
            "refusing to merge unrelated histories; run `mg fetch` then merge manually",
        )),
    }
}

/// 与 `git pull` 的 fast-forward 同序：先校验 + 物化 + 重建 index，最后 CAS 移动分支引用。
fn fast_forward(repo: &Repo, branch: &str, head: Oid, target: Oid) -> Result<()> {
    let previous = Index::read(repo)?;
    if previous.has_conflicts() {
        return Err(Error::Other(
            "you need to resolve your current index first".to_string(),
        ));
    }
    let entries = tree_entries(repo, commit_tree(repo, target)?)?;
    let applied = apply_tree(repo, &entries, false)?;
    let written: BTreeSet<Vec<u8>> = applied.written.into_iter().collect();
    tree_index(repo, &entries, &previous, Some(&written))?.write(repo)?;
    RefStore::new(repo).update(&full_ref(branch), target, Some(Some(head)))?;
    println!("Fast-forward");
    Ok(())
}
