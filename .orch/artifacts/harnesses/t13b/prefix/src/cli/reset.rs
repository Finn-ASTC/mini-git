//! `mg reset` —— **T11（omp）**。
//!
//! * `--soft`：只把 HEAD（或 attached 的分支引用）挪到目标提交，index 与工作区一个字节都不动
//!   （实测：git 的 `reset --soft` 连 index 文件内容都不变）。
//! * `--mixed`（默认）：再按目标 tree 重建 index；工作区不动。改动过的条目 stat 清零，
//!   于是 git 会重新哈希内容、照样看出 ` M`（与 git 的 `reset --mixed` 逐字段一致）。
//! * `--hard`：再以 `force = true` 物化（未跟踪 / ignored 文件保留），index 取落盘后的 stat，
//!   因此收尾时 `git status --porcelain` 为空。
//! * 顺带写 `.git/ORIG_HEAD`：**40 位十六进制 + `\n`**，值是重置前的 HEAD 提交。
//!
//! 引用更新走 `RefStore::update` 的 CAS：并发写者插队时报 `RefConflict` 而不是静默覆盖。

use std::collections::BTreeSet;
use std::fs;

use crate::cli::{open_repo, ResetMode};
use crate::error::{Error, Result};
use crate::index::Index;
use crate::oid::Oid;
use crate::refs::{Head, RefStore};
use crate::repo::Repo;
use crate::worktree::materialize::{
    apply_tree, commit_tree, head_commit, resolve_commit, tree_entries, tree_index,
};

pub fn run(mode: ResetMode, rev: &str) -> Result<()> {
    let repo = open_repo()?;
    let refs = RefStore::new(&repo);
    let target = resolve_commit(&repo, rev)?;
    let head = refs.read_head()?;

    if let Some(previous) = head_commit(&repo)? {
        write_orig_head(&repo, previous)?;
    }

    // 先读旧 index（`--hard` 的「已跟踪」集合来自它），再动 index / 工作区，
    // 最后才挪引用：中途失败时 HEAD 还指着原处，不会留下「HEAD 走了、文件没跟上」的半成品。
    let previous = Index::read(&repo)?;
    match mode {
        ResetMode::Soft => {}
        ResetMode::Mixed => {
            let entries = tree_entries(&repo, commit_tree(&repo, target)?)?;
            tree_index(&repo, &entries, &previous, None)?.write(&repo)?;
        }
        ResetMode::Hard => {
            let entries = tree_entries(&repo, commit_tree(&repo, target)?)?;
            let applied = apply_tree(&repo, &entries, true)?;
            let written: BTreeSet<Vec<u8>> = applied.written.into_iter().collect();
            tree_index(&repo, &entries, &previous, Some(&written))?.write(&repo)?;
        }
    }
    move_head(&refs, &head, target)?;
    if mode == ResetMode::Hard {
        print_head_at(&repo, target);
    }
    Ok(())
}

/// `--soft` / `--mixed` / `--hard` 的第一步：把 HEAD 指向目标提交。
fn move_head(refs: &RefStore<'_>, head: &Head, target: Oid) -> Result<()> {
    match head {
        Head::Attached(name) => {
            let current = match refs.resolve(name) {
                Ok(oid) => Some(oid),
                Err(Error::RefNotFound(_)) => None,
                Err(err) => return Err(err),
            };
            // CAS：`Some(Some(oid))` = 旧值必须匹配，`None` = 引用必须还不存在（unborn HEAD）。
            refs.update(name, target, current.map(Some))
        }
        Head::Detached(_) => refs.set_head_detached(target),
    }
}

/// `ORIG_HEAD` = 重置前的 HEAD 提交，十六进制 + `\n`（41 字节，与 git 相同）。
fn write_orig_head(repo: &Repo, previous: Oid) -> Result<()> {
    let body = format!("{}\n", previous.to_hex());
    fs::write(repo.git_dir().join("ORIG_HEAD"), body)?;
    Ok(())
}

/// `HEAD is now at <7 位> <subject>`（与 git 的措辞一致，仅用于人读）。
fn print_head_at(repo: &Repo, commit: Oid) {
    let subject = crate::odb::Odb::new(repo)
        .read_object(commit)
        .ok()
        .and_then(|object| match object {
            crate::object::Object::Commit(commit) => Some(commit.message),
            _ => None,
        })
        .map(|message| {
            let line = message.split(|byte| *byte == b'\n').next().unwrap_or(&[]);
            String::from_utf8_lossy(line).trim_end().to_string()
        })
        .unwrap_or_default();
    let hex = commit.to_hex();
    if subject.is_empty() {
        println!("HEAD is now at {}", &hex[..7]);
    } else {
        println!("HEAD is now at {} {subject}", &hex[..7]);
    }
}
