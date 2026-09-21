//! `mg rm` —— **T12（hermes）**。
//!
//! 默认：删 index 条目 **并且** 删工作区文件（顺带清掉变空的父目录，与 git 一致）；
//! `--cached`：只删 index 条目（工作区与 HEAD 都不动）。
//!
//! ## 拒绝规则（与真实 git 对拍得到，git 2.55）
//! | 场景 | 不带 `-f` 的结果 |
//! |---|---|
//! | 工作区内容/mode 与 index 不同 | **拒绝**（`Error::WouldLoseChanges`，零改动）|
//! | 工作区文件已不存在 | 允许（git 也照删 index 条目）|
//! | index 与 HEAD 不同（已暂存）| **拒绝** |
//! | `--cached` 且「暂存内容既不同于工作区也不同于 HEAD」| **拒绝** |
//! | `--cached` 且 index 与 HEAD 不同但工作区与 index 相同 | 允许 |
//! | pathspec 命中不到任何 index 条目（含未被跟踪的文件）| `Error::PathspecNotFound` |
//!
//! 拒绝必须**零改动**：所有校验都在动手之前跑完（本轮验收的反例之一）。
//!
//! 层内共享的小工具（`expand_pathspecs` / `invalidate_tree_cache`）来自
//! `cli::add`，见那里的文件头说明（cache-tree 失效是本轮的实测教训）。

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry};
use crate::oid::Oid;
use crate::repo::Repo;
use crate::worktree::Worktree;

use super::add::{expand_pathspecs, invalidate_tree_cache};

pub fn run(pathspec: &[PathBuf], cached: bool, force: bool) -> Result<()> {
    let repo = crate::cli::open_repo()?;
    let worktree = Worktree::new(&repo);
    let mut index = Index::read(&repo)?;
    let specs = expand_pathspecs(&repo, pathspec, &index)?;

    // 1) 展开到 index 里被命中的路径（含冲突的 stage 1/2/3）。
    let mut targets: Vec<Vec<u8>> = index
        .entries
        .iter()
        .filter(|entry| specs.iter().any(|spec| spec.matches(&entry.path)))
        .map(|entry| entry.path.clone())
        .collect();
    targets.sort();
    targets.dedup();
    if targets.is_empty() {
        let raw = specs
            .iter()
            .find(|spec| !spec.matches_anything(&repo, &index).unwrap_or(true))
            .or_else(|| specs.first())
            .map(|spec| spec.raw.clone())
            .unwrap_or_default();
        return Err(Error::PathspecNotFound(raw));
    }

    // 2) 目录要先 `-r`，而签名里没有 `-r`：明确拒绝，不偷偷递归。
    for spec in &specs {
        if spec.is_dir_in_worktree(&repo) && targets.iter().any(|path| spec.matches(path)) {
            return Err(Error::Other(format!(
                "not removing '{}' recursively without -r (mini-git `rm` has no -r)",
                spec.raw
            )));
        }
    }

    let head = super::status::head_tree_map(&repo)?;

    // 3) 校验（全部跑完才动手，保证拒绝时零改动）。
    let mut local_mods: Vec<String> = Vec::new();
    let mut staged: Vec<String> = Vec::new();
    if !force {
        for path in &targets {
            let entry = index.lookup(path);
            let conflicted = index
                .entries
                .iter()
                .any(|other| other.path == *path && other.stage != 0);
            let worktree_same = match entry {
                Some(entry) => worktree_matches_entry(&worktree, path, entry)?,
                None => true,
            };
            let index_same_as_head = match (head.as_ref(), entry) {
                (Some(map), Some(entry)) => {
                    map.get(path) == Some(&(entry.oid, entry.mode))
                }
                (None, None) => true,
                _ => false,
            };
            let name = String::from_utf8_lossy(path).into_owned();
            if conflicted {
                staged.push(name);
            } else if cached {
                // git：只有「暂存内容与工作区、HEAD 都不同」才拒绝。
                if !index_same_as_head && !worktree_same {
                    staged.push(name);
                }
            } else if !worktree_same {
                local_mods.push(name);
            } else if !index_same_as_head {
                staged.push(name);
            }
        }
    }

    if !local_mods.is_empty() {
        return Err(Error::WouldLoseChanges(format!(
            "the following file has local modifications: {}\n(use --cached to keep the file, or -f to force removal)",
            local_mods.join(", ")
        )));
    }
    if !staged.is_empty() {
        let hint = if cached {
            "(use -f to force removal)"
        } else {
            "(use --cached to keep the file, or -f to force removal)"
        };
        let what = if cached {
            "the following file has staged content different from both the file and the HEAD"
        } else {
            "the following file has changes staged in the index"
        };
        return Err(Error::WouldLoseChanges(format!(
            "{what}: {}\n{hint}",
            staged.join(", ")
        )));
    }

    // 4) 执行。
    for path in &targets {
        index.remove_all_stages(path);
        if !cached {
            remove_worktree_file(&repo, path)?;
        }
        println!("rm '{}'", String::from_utf8_lossy(path));
    }
    invalidate_tree_cache(&mut index);
    index.write(&repo)
}

/// 工作区里的文件是否与 index 条目一致（缺失算「一致」，与 git 的 `rm` 行为一致）。
fn worktree_matches_entry(
    worktree: &Worktree<'_>,
    path: &[u8],
    entry: &IndexEntry,
) -> Result<bool> {
    let abs = worktree.repo().work_path(path)?;
    match std::fs::symlink_metadata(&abs) {
        Err(_) => Ok(true),
        Ok(meta) if meta.is_dir() => Ok(false),
        Ok(_) => {
            let content = worktree.read_worktree_entry(path)?;
            Ok(content.mode == entry.mode
                && Oid::hash_object("blob", &content.bytes) == entry.oid)
        }
    }
}

/// 删工作区文件并清理因此变空的父目录（真实 `git rm d/e/x.txt` 也会删掉 `d/e/`）。
fn remove_worktree_file(repo: &Repo, path: &[u8]) -> Result<()> {
    let abs = repo.work_path(path)?;
    match std::fs::symlink_metadata(&abs) {
        Ok(meta) if meta.is_dir() => {
            return Err(Error::Other(format!(
                "not removing '{}': it is a directory",
                String::from_utf8_lossy(path)
            )))
        }
        Ok(_) => std::fs::remove_file(&abs)?,
        Err(_) => return Ok(()),
    }
    prune_empty_dirs(repo, path);
    Ok(())
}

/// 自下而上删掉变空的父目录；`remove_dir` 对非空目录失败，天然停在正确的位置。
fn prune_empty_dirs(repo: &Repo, path: &[u8]) {
    let Some(workdir) = repo.workdir() else {
        return;
    };
    let mut end = path.len();
    while let Some(offset) = path[..end].iter().rposition(|byte| *byte == b'/') {
        let prefix = &path[..offset];
        let Ok(directory) = std::str::from_utf8(prefix) else {
            return;
        };
        if std::fs::remove_dir(workdir.join(directory)).is_err() {
            return;
        }
        end = offset;
    }
}
