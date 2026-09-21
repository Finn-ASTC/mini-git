//! `mg checkout` —— **T11（omp）**。
//!
//! 两种形态：
//!
//! 1. **切提交/分支**（`mg checkout <rev> [--detach] [-f]`）
//!    → 物化目标 tree + 重建 index + 改 HEAD。
//!    `rev` 正好是本地分支名时附加到该分支，否则 detach（`--detach` 强制 detach）——
//!    与 git 相同（tag / sha / `HEAD` 都 detach）。已在同一分支且没有 `-f` 时什么都不做
//!    （git: `Already on '<branch>'`；即使 index 有暂存改动也不动，实测 E16）。
//! 2. **取回路径**（`mg checkout <rev> -- <path>…`）
//!    → 只把被 pathspec 选中的路径从 `rev` 写回工作区 + index，其余路径一个字节都不动
//!    （实测 E28/E33：`rev` 里没有的路径不会被删，未被 pathspec 命中的路径不会被改）。
//!    与 git 一样，**这一形态会丢弃这些路径上的本地改动**（不需要 `-f`，实测 E6a）；
//!    pathspec 一个都没命中 → `PathspecNotFound`（git: `pathspec … did not match`）。
//!
//! 安全：`rev` 形态下会被覆盖的本地改动 → `Error::WouldLoseChanges`，且工作区 / index / HEAD
//! 全都不动；index 里还有未解决的冲突时直接拒绝（git: `you need to resolve your current index first`，
//! 加了 `-f` 则照做并清掉冲突）。目标提交恰好等于当前 HEAD 提交且没有 `-f` 时，git 只改 HEAD、
//! 不碰 index 与工作区（实测 P1/P2/P8/P9/P10），这条捷径在 `materialize::checkout_to` 里。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::open_repo;
use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry, StatData, TREE_EXTENSION};
use crate::object::FileMode;
use crate::oid::Oid;
use crate::refs::RefStore;
use crate::repo::Repo;
use crate::worktree::materialize::{
    checkout_to, commit_tree, current_branch, full_ref, prepare_write_target, resolve_commit,
    tree_entries, worktree_path, write_blob_to, HeadTarget,
};

pub fn run(rev: &str, paths: &[PathBuf], force: bool, detach: bool) -> Result<()> {
    let repo = open_repo()?;
    if !paths.is_empty() {
        return checkout_paths(&repo, rev, paths);
    }
    checkout_rev(&repo, rev, force, detach)
}

fn checkout_rev(repo: &Repo, rev: &str, force: bool, detach: bool) -> Result<()> {
    let commit = resolve_commit(repo, rev)?;
    let refs = RefStore::new(repo);
    let attach = !detach && refs.exists(&full_ref(rev));
    if attach {
        if !force && current_branch(repo)?.as_deref() == Some(rev) {
            println!("Already on '{rev}'");
            return Ok(());
        }
        checkout_to(repo, commit, force, HeadTarget::Attached(rev))?;
        println!("Switched to branch '{rev}'");
        return Ok(());
    }
    checkout_to(repo, commit, force, HeadTarget::Detached(commit))?;
    let hex = commit.to_hex();
    println!("HEAD is now at {} ({})", &hex[..7], rev);
    Ok(())
}

fn checkout_paths(repo: &Repo, rev: &str, paths: &[PathBuf]) -> Result<()> {
    let commit = resolve_commit(repo, rev)?;
    let entries = tree_entries(repo, commit_tree(repo, commit)?)?;
    let filters: Vec<Vec<u8>> = paths
        .iter()
        .map(|path| path_bytes(path.as_path()))
        .collect();

    // git 会对每个没命中的 pathspec 报错（`error: pathspec 'x' did not match any file(s) known to git`）。
    for (filter, original) in filters.iter().zip(paths) {
        if !entries.iter().any(|entry| matches_path(&entry.0, filter)) {
            return Err(Error::PathspecNotFound(original.display().to_string()));
        }
    }

    let selected: Vec<&(Vec<u8>, Oid, FileMode)> = entries
        .iter()
        .filter(|entry| filters.iter().any(|filter| matches_path(&entry.0, filter)))
        .collect();

    let mut index = Index::read(repo)?;
    let updated_paths: BTreeSet<&[u8]> = selected.iter().map(|entry| entry.0.as_slice()).collect();
    let mut fresh = Vec::with_capacity(selected.len());
    for entry in &selected {
        // 路径形态的 checkout 有意丢弃这些路径上的本地改动（与 git 一致）。
        prepare_write_target(repo, &entry.0, true)?;
        write_blob_to(repo, &entry.0, entry.1, entry.2)?;
        let mut updated = IndexEntry::new(entry.0.clone(), entry.1, entry.2);
        updated.stat =
            StatData::from_metadata(&fs::symlink_metadata(worktree_path(repo, &entry.0)?)?);
        fresh.push(updated);
    }
    // 一次到位：先剔除被写路径的全部 stage（含冲突 1/2/3），再追加新条目、整体排序。
    index
        .entries
        .retain(|existing| !updated_paths.contains(existing.path.as_slice()));
    index.entries.extend(fresh);
    index.sort();
    // 条目变了，cache-tree 缓存必然失效（不重建，只丢弃）。
    index.tree_oid = None;
    index
        .extensions
        .retain(|ext| ext.signature != TREE_EXTENSION);
    index.write(repo)?;
    Ok(())
}

/// pathspec 命中：精确相等，或位于该目录之下（`dir` 命中 `dir/x`，但不命中 `dirs`）。
fn matches_path(path: &[u8], filter: &[u8]) -> bool {
    path == filter
        || (path.len() > filter.len() && path.starts_with(filter) && path[filter.len()] == b'/')
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}
