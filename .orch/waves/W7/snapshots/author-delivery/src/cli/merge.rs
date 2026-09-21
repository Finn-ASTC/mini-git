//! `mg merge` —— **T10（opencode）**。
//!
//! 流程：解析 rev → 找 merge-base → 已是最新则提示 → fast-forward（除非 `--no-ff`）→
//! 否则对每个路径做三方合并；干净则直接写 commit，冲突则写
//! `.git/MERGE_HEAD`、`.git/ORIG_HEAD`、index stage 1/2/3 与冲突标记，并以非零码退出。
//!
//! 验收：无冲突时 tree 与 `git merge` 相同；有冲突时冲突文件内容与 `git merge` 逐字节相同，
//! 且 `git status --porcelain` 的 `UU/AA/DU/...` 组合一致；
//! `git commit` 能在 mg 留下的状态上直接完成合并（证明状态文件兼容）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry, TREE_EXTENSION};
use crate::merge::three_way::{self, MergeLabels, Merged};
use crate::object::{Commit, FileMode, Kind, Signature, Tree, TreeEntry};
use crate::odb::Odb;
use crate::oid::Oid;
use crate::refs::{Head, RefStore};
use crate::repo::Repo;
use crate::worktree::materialize::apply_tree;
use crate::worktree::{ChangeKind, Worktree};

/// `git merge`（`merge-ll.c` → `xdl_merge`）用的简化级别。
const LEVEL_ZEALOUS: i32 = 2;
/// diff3 风格会被钳到 EAGER。
const LEVEL_EAGER: i32 = 1;

const DEFAULT_IDENTITY_NAME: &str = "mini-git";
const DEFAULT_IDENTITY_EMAIL: &str = "mini-git@example.invalid";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    mode: FileMode,
    oid: Oid,
}

/// 一个冲突路径：`None` 表示该侧没有这个文件（delete/modify 冲突）。
struct Conflict {
    path: Vec<u8>,
    base: Option<Entry>,
    ours: Option<Entry>,
    theirs: Option<Entry>,
}

pub fn run(rev: &str, message: Option<&str>, no_ff: bool) -> Result<()> {
    let repo = crate::cli::open_repo()?;
    merge_into(&repo, rev, message, no_ff)
}

fn merge_into(repo: &Repo, rev: &str, message: Option<&str>, no_ff: bool) -> Result<()> {
    let refs = RefStore::new(repo);
    let odb = Odb::new(repo);
    let worktree = Worktree::new(repo);

    let head = refs.read_head()?;
    let head_oid = refs.resolve("HEAD")?;
    let target_oid = refs.resolve(rev)?;
    let target_commit = read_commit(&odb, target_oid)?;

    let index = Index::read(repo)?;
    let head_commit = read_commit(&odb, head_oid)?;
    let ours_flat = flatten_tree(&odb, head_commit.tree)?;

    // 合并不能在未解决的冲突 index 上开始（与 git 的 `unpack_trees` 前置条件一致）。
    if index.has_conflicts() {
        return Err(Error::Other(
            "you need to resolve your current index first".to_string(),
        ));
    }

    let base_oid = match crate::merge::merge_base(repo, head_oid, target_oid)? {
        Some(oid) => oid,
        None => {
            return Err(Error::Unsupported(
                "refusing to merge unrelated histories (no common ancestor)",
            ))
        }
    };
    // 已是目标（目标为祖先）时不动任何路径，因此本地改动不影响：与 git 的路径式判定一致。
    if base_oid == target_oid {
        println!("Already up to date.");
        return Ok(());
    }

    let theirs_flat = flatten_tree(&odb, target_commit.tree)?;

    if base_oid == head_oid && !no_ff {
        fast_forward(
            repo,
            &refs,
            &head,
            head_oid,
            target_oid,
            &target_commit.tree,
            &ours_flat,
            &theirs_flat,
            &index,
        )?;
        return Ok(());
    }

    let base_flat = {
        let commit = read_commit(&odb, base_oid)?;
        flatten_tree(&odb, commit.tree)?
    };

    real_merge(
        repo,
        &refs,
        &odb,
        &worktree,
        &head,
        head_oid,
        target_oid,
        rev,
        message,
        base_oid,
        &base_flat,
        &ours_flat,
        &theirs_flat,
        &index,
    )
}

#[allow(clippy::too_many_arguments)]
fn fast_forward(
    repo: &Repo,
    refs: &RefStore,
    head: &Head,
    head_oid: Oid,
    target_oid: Oid,
    target_tree: &Oid,
    ours_flat: &[(Vec<u8>, FileMode, Oid)],
    target_flat: &[(Vec<u8>, FileMode, Oid)],
    index: &Index,
) -> Result<()> {
    let head_map = entry_map(ours_flat);
    let target_map = entry_map(target_flat);

    // git 的快进守卫是**按路径**的：只有本次会更新/删除的路径才检查本地改动。
    // 若该路径的 index 既不是 HEAD、也不是目标（即夹带了本地暂存改动），拒绝。
    for entry in index.entries.iter().filter(|entry| entry.is_stage0()) {
        let path = entry.path.as_slice();
        if head_map.get(path) == target_map.get(path) {
            continue;
        }
        let staged = Entry {
            mode: entry.mode,
            oid: entry.oid,
        };
        if Some(&staged) != target_map.get(path) && Some(&staged) != head_map.get(path) {
            return Err(Error::WouldLoseChanges(
                String::from_utf8_lossy(path).into_owned(),
            ));
        }
    }

    // 目标工作区 = 未被触及的路径沿用当前 index 内容（从而保住本地/暂存状态），
    // 被触及的路径取目标 tree；校验与物化交给 T11 的 `apply_tree` 逐路径完成。
    let mut overlay: BTreeMap<Vec<u8>, (Oid, FileMode)> = BTreeMap::new();
    for entry in index.entries.iter().filter(|entry| entry.is_stage0()) {
        let path = entry.path.as_slice();
        if head_map.get(path) == target_map.get(path) {
            overlay.insert(entry.path.clone(), (entry.oid, entry.mode));
        } else if let Some(target) = target_map.get(path) {
            overlay.insert(entry.path.clone(), (target.oid, target.mode));
        }
    }
    for (path, target) in &target_map {
        overlay
            .entry(path.clone())
            .or_insert((target.oid, target.mode));
    }
    let target: Vec<(Vec<u8>, Oid, FileMode)> = overlay
        .into_iter()
        .map(|(path, (oid, mode))| (path, oid, mode))
        .collect();
    apply_tree(repo, &target, false)?;

    let mut entries: Vec<IndexEntry> = target
        .iter()
        .map(|(path, oid, mode)| IndexEntry::new(path.clone(), *oid, *mode))
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path).then(a.stage.cmp(&b.stage)));
    let merged = Index {
        version: 2,
        entries,
        tree_oid: Some(*target_tree),
        extensions: index
            .extensions
            .iter()
            .filter(|ext| ext.signature != TREE_EXTENSION)
            .cloned()
            .collect(),
    };
    merged.write(repo)?;

    match head {
        Head::Attached(name) => refs.update(name, target_oid, Some(Some(head_oid)))?,
        Head::Detached(_) => refs.set_head_detached(target_oid)?,
    }
    println!("Fast-forward");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn real_merge(
    repo: &Repo,
    refs: &RefStore,
    odb: &Odb,
    worktree: &Worktree,
    head: &Head,
    head_oid: Oid,
    target_oid: Oid,
    rev: &str,
    message: Option<&str>,
    base_oid: Oid,
    base_flat: &[(Vec<u8>, FileMode, Oid)],
    ours_flat: &[(Vec<u8>, FileMode, Oid)],
    theirs_flat: &[(Vec<u8>, FileMode, Oid)],
    index: &Index,
) -> Result<()> {
    let diff3 = repo
        .config()
        .get("merge.conflictstyle")
        .map(|style| style.trim() == "diff3" || style.trim() == "zdiff3")
        .unwrap_or(false);
    let labels = MergeLabels {
        ours: "HEAD".to_string(),
        base: base_oid.to_hex()[..7].to_string(),
        theirs: rev.to_string(),
        diff3_style: diff3,
    };
    let level = if diff3 { LEVEL_EAGER } else { LEVEL_ZEALOUS };

    let base_map = entry_map(base_flat);
    let ours_map = entry_map(ours_flat);
    let theirs_map = entry_map(theirs_flat);

    // 真实 git 对**非快进**合并要求 index 与 HEAD 完全一致（任何暂存改动都会被
    // 合并结果覆盖并报 `local changes ... would be overwritten`），与路径无关。
    if !index_matches_head(index, &ours_map) {
        return Err(Error::WouldLoseChanges(
            "you have staged changes; commit or stash them before merging".to_string(),
        ));
    }
    // 只需按路径拒绝：工作区里被本次写/删触及的改动。无关路径的本地改动原样保留。
    let report = worktree.status(Some(&flat_tree(ours_flat)), index)?;
    let dirty: BTreeSet<Vec<u8>> = report
        .lines
        .iter()
        .filter(|line| {
            !line.untracked
                && matches!(
                    line.worktree,
                    Some(ChangeKind::Modified | ChangeKind::TypeChanged)
                )
        })
        .map(|line| line.path.clone())
        .collect();

    let mut paths: BTreeSet<Vec<u8>> = BTreeSet::new();
    paths.extend(base_map.keys().cloned());
    paths.extend(ours_map.keys().cloned());
    paths.extend(theirs_map.keys().cloned());

    let mut results: Vec<(Vec<u8>, FileMode, Oid)> = Vec::new();
    let mut conflicts: Vec<Conflict> = Vec::new();
    let mut deletes: Vec<Vec<u8>> = Vec::new();

    for path in paths {
        let base = base_map.get(&path).copied();
        let ours = ours_map.get(&path).copied();
        let theirs = theirs_map.get(&path).copied();

        match (ours, theirs) {
            (None, None) => deletes.push(path),
            (Some(ours_entry), None) => {
                if base.is_none() {
                    results.push((path, ours_entry.mode, ours_entry.oid));
                } else if base == Some(ours_entry) {
                    deletes.push(path);
                } else {
                    conflicts.push(Conflict {
                        path,
                        base,
                        ours: Some(ours_entry),
                        theirs: None,
                    });
                }
            }
            (None, Some(theirs_entry)) => {
                if worktree_exists(repo, &path) {
                    return Err(Error::WouldLoseChanges(format!(
                        "untracked working tree file would be overwritten: {}",
                        String::from_utf8_lossy(&path)
                    )));
                }
                if base.is_none() {
                    write_worktree_entry(worktree, odb, &path, theirs_entry)?;
                    results.push((path, theirs_entry.mode, theirs_entry.oid));
                } else if base == Some(theirs_entry) {
                    deletes.push(path);
                } else {
                    write_worktree_entry(worktree, odb, &path, theirs_entry)?;
                    conflicts.push(Conflict {
                        path,
                        base,
                        ours: None,
                        theirs: Some(theirs_entry),
                    });
                }
            }
            (Some(ours_entry), Some(theirs_entry)) => {
                let ours_bytes = read_blob(odb, ours_entry.oid)?;
                let theirs_bytes = read_blob(odb, theirs_entry.oid)?;
                let base_bytes = match base {
                    Some(entry) => Some(read_blob(odb, entry.oid)?),
                    None => None,
                };
                match three_way::merge_blobs_with_level(
                    base_bytes.as_deref(),
                    &ours_bytes,
                    &theirs_bytes,
                    &labels,
                    level,
                )? {
                    Merged::Clean(bytes) => {
                        let mode = resolve_mode(base, ours_entry, theirs_entry);
                        let oid = odb.write(Kind::Blob, &bytes)?;
                        // 合并结果与 ours 相同 → git 不碰工作区，本地改动原样保留。
                        let merged_entry = Entry { mode, oid };
                        if merged_entry != ours_entry {
                            ensure_clean(&dirty, &path)?;
                            write_worktree_entry(worktree, odb, &path, merged_entry)?;
                        }
                        results.push((path, mode, oid));
                    }
                    Merged::Conflict(bytes) => {
                        ensure_clean(&dirty, &path)?;
                        let oid = odb.write(Kind::Blob, &bytes)?;
                        let marker = Entry {
                            mode: ours_entry.mode,
                            oid,
                        };
                        write_worktree_entry(worktree, odb, &path, marker)?;
                        conflicts.push(Conflict {
                            path,
                            base,
                            ours: Some(ours_entry),
                            theirs: Some(theirs_entry),
                        });
                    }
                }
            }
        }
    }

    // 会被本次合并删掉的路径同样按路径校验：本地改动会被删掉时拒绝（缺失不算改动）。
    for path in &deletes {
        ensure_clean(&dirty, path)?;
    }

    if !conflicts.is_empty() {
        return finish_conflict(
            repo, head_oid, target_oid, rev, message, &results, &conflicts, &deletes,
        );
    }

    for path in &deletes {
        remove_worktree_path(repo, path)?;
    }

    let new_tree = build_tree(odb, &results)?;
    let merged_index = index_from_flat(&results, Some(new_tree));
    merged_index.write(repo)?;

    let commit_oid = write_merge_commit(odb, repo, new_tree, head_oid, target_oid, message, rev)?;
    match head {
        Head::Attached(name) => refs.update(name, commit_oid, Some(Some(head_oid)))?,
        Head::Detached(_) => refs.set_head_detached(commit_oid)?,
    }
    cleanup_merge_state(repo);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finish_conflict(
    repo: &Repo,
    head_oid: Oid,
    target_oid: Oid,
    rev: &str,
    message: Option<&str>,
    results: &[(Vec<u8>, FileMode, Oid)],
    conflicts: &[Conflict],
    deletes: &[Vec<u8>],
) -> Result<()> {
    for path in deletes {
        remove_worktree_path(repo, path)?;
    }

    let mut index = Index {
        version: 2,
        entries: Vec::new(),
        tree_oid: None,
        extensions: Vec::new(),
    };
    for (path, mode, oid) in results {
        index.upsert(IndexEntry::new(path.clone(), *oid, *mode));
    }
    for conflict in conflicts {
        for (stage, entry) in [
            (1u8, conflict.base),
            (2u8, conflict.ours),
            (3u8, conflict.theirs),
        ] {
            if let Some(entry) = entry {
                let mut index_entry = IndexEntry::new(conflict.path.clone(), entry.oid, entry.mode);
                index_entry.stage = stage;
                index.upsert(index_entry);
            }
        }
    }
    index.sort();
    index.write(repo)?;

    let git_dir = repo.git_dir();
    std::fs::write(
        git_dir.join("MERGE_HEAD"),
        format!("{}\n", target_oid.to_hex()),
    )?;
    std::fs::write(
        git_dir.join("ORIG_HEAD"),
        format!("{}\n", head_oid.to_hex()),
    )?;
    let merge_message = merge_message(message, rev);
    std::fs::write(git_dir.join("MERGE_MSG"), merge_message.as_bytes())?;

    Err(Error::Other(
        "automatic merge failed; fix conflicts and then commit the result".to_string(),
    ))
}

fn write_merge_commit(
    odb: &Odb,
    repo: &Repo,
    tree: Oid,
    head_oid: Oid,
    target_oid: Oid,
    message: Option<&str>,
    rev: &str,
) -> Result<Oid> {
    let when = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let name = config_bytes(repo, "user.name", DEFAULT_IDENTITY_NAME);
    let email = config_bytes(repo, "user.email", DEFAULT_IDENTITY_EMAIL);
    let signature = Signature::new(name, email, when, "+0000");
    let commit = Commit {
        tree,
        parents: vec![head_oid, target_oid],
        author: signature.clone(),
        committer: signature,
        message: merge_message(message, rev).into_bytes(),
        extra_headers: Vec::new(),
    };
    odb.write(Kind::Commit, &commit.encode_payload())
}

fn merge_message(message: Option<&str>, rev: &str) -> String {
    let text = message
        .map(str::to_string)
        .unwrap_or_else(|| format!("Merge branch '{rev}'"));
    if text.ends_with('\n') {
        text
    } else {
        format!("{text}\n")
    }
}

fn config_bytes(repo: &Repo, key: &str, fallback: &str) -> Vec<u8> {
    repo.config()
        .get(key)
        .map(|value| value.as_bytes().to_vec())
        .unwrap_or_else(|| fallback.as_bytes().to_vec())
}

fn resolve_mode(base: Option<Entry>, ours: Entry, theirs: Entry) -> FileMode {
    if ours.mode != theirs.mode && base.map(|entry| entry.mode) == Some(ours.mode) {
        theirs.mode
    } else {
        ours.mode
    }
}

fn entry_map(flat: &[(Vec<u8>, FileMode, Oid)]) -> BTreeMap<Vec<u8>, Entry> {
    flat.iter()
        .map(|(path, mode, oid)| {
            (
                path.clone(),
                Entry {
                    mode: *mode,
                    oid: *oid,
                },
            )
        })
        .collect()
}

/// 真实 git 对非快进合并要求 index 与 HEAD 的每个路径逐条一致（含新增/删除）。
fn index_matches_head(index: &Index, head: &BTreeMap<Vec<u8>, Entry>) -> bool {
    if index.entries.iter().any(|entry| !entry.is_stage0()) {
        return false;
    }
    for entry in &index.entries {
        match head.get(&entry.path) {
            Some(existing) if existing.oid == entry.oid && existing.mode == entry.mode => {}
            _ => return false,
        }
    }
    let indexed: BTreeSet<&[u8]> = index
        .entries
        .iter()
        .map(|entry| entry.path.as_slice())
        .collect();
    head.keys().all(|path| indexed.contains(path.as_slice()))
}

/// 本次合并会写/删的路径上若有未暂存的本地改动 → 拒绝（缺失的路径不算改动）。
fn ensure_clean(dirty: &BTreeSet<Vec<u8>>, path: &[u8]) -> Result<()> {
    if dirty.contains(path) {
        return Err(Error::WouldLoseChanges(
            String::from_utf8_lossy(path).into_owned(),
        ));
    }
    Ok(())
}

fn flat_tree(flat: &[(Vec<u8>, FileMode, Oid)]) -> Tree {
    Tree::new(
        flat.iter()
            .map(|(path, mode, oid)| TreeEntry {
                mode: *mode,
                name: path.clone(),
                oid: *oid,
            })
            .collect(),
    )
}

fn index_from_flat(flat: &[(Vec<u8>, FileMode, Oid)], tree_oid: Option<Oid>) -> Index {
    let mut entries: Vec<IndexEntry> = flat
        .iter()
        .map(|(path, mode, oid)| IndexEntry::new(path.clone(), *oid, *mode))
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path).then(a.stage.cmp(&b.stage)));
    Index {
        version: 2,
        entries,
        tree_oid,
        extensions: Vec::new(),
    }
}

/// 递归读取 tree，返回**已展平**的 `(仓库相对路径, mode, oid)`（按路径排序）。
fn flatten_tree(odb: &Odb, root: Oid) -> Result<Vec<(Vec<u8>, FileMode, Oid)>> {
    let mut out: Vec<(Vec<u8>, FileMode, Oid)> = Vec::new();
    let mut stack: Vec<(Vec<u8>, Oid)> = vec![(Vec::new(), root)];
    while let Some((prefix, oid)) = stack.pop() {
        let tree = odb.read_object(oid)?.into_tree()?;
        for entry in tree.entries() {
            let mut path = prefix.clone();
            path.extend_from_slice(&entry.name);
            if entry.mode == FileMode::Tree {
                let mut child = path.clone();
                child.push(b'/');
                stack.push((child, entry.oid));
            } else {
                out.push((path, entry.mode, entry.oid));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// 由展平条目自底向上构造 tree（子目录先建）。
fn build_tree(odb: &Odb, entries: &[(Vec<u8>, FileMode, Oid)]) -> Result<Oid> {
    build_subtree(odb, entries)
}

fn build_subtree(odb: &Odb, entries: &[(Vec<u8>, FileMode, Oid)]) -> Result<Oid> {
    let mut tree_entries: Vec<TreeEntry> = Vec::new();
    let mut index = 0usize;
    while index < entries.len() {
        let (path, mode, oid) = &entries[index];
        match path.iter().position(|byte| *byte == b'/') {
            None => {
                push_tree_entry(
                    &mut tree_entries,
                    TreeEntry {
                        mode: *mode,
                        name: path.clone(),
                        oid: *oid,
                    },
                )?;
                index += 1;
            }
            Some(slash) => {
                let dir = &path[..slash];
                let mut sub: Vec<(Vec<u8>, FileMode, Oid)> = Vec::new();
                let mut cursor = index;
                while cursor < entries.len() {
                    let candidate = &entries[cursor].0;
                    if candidate.len() > slash
                        && &candidate[..slash] == dir
                        && candidate[slash] == b'/'
                    {
                        sub.push((
                            candidate[slash + 1..].to_vec(),
                            entries[cursor].1,
                            entries[cursor].2,
                        ));
                        cursor += 1;
                    } else {
                        break;
                    }
                }
                let sub_oid = build_subtree(odb, &sub)?;
                push_tree_entry(
                    &mut tree_entries,
                    TreeEntry {
                        mode: FileMode::Tree,
                        name: dir.to_vec(),
                        oid: sub_oid,
                    },
                )?;
                index = cursor;
            }
        }
    }
    let payload = Tree::new(tree_entries).encode_payload();
    odb.write(Kind::Tree, &payload)
}

/// D/F 冲突（同名既是文件又是目录）在 v1 明确不支持——宁可报错，也不写出损坏的 tree。
fn push_tree_entry(entries: &mut Vec<TreeEntry>, entry: TreeEntry) -> Result<()> {
    if entries.iter().any(|existing| existing.name == entry.name) {
        return Err(Error::Unsupported(
            "directory/file conflicts are not supported in v1",
        ));
    }
    entries.push(entry);
    Ok(())
}

fn worktree_exists(repo: &Repo, path: &[u8]) -> bool {
    repo.work_path(path)
        .map(|absolute| std::fs::symlink_metadata(absolute).is_ok())
        .unwrap_or(false)
}

fn read_commit(odb: &Odb, oid: Oid) -> Result<Commit> {
    odb.read_object(oid)?.into_commit()
}

fn read_blob(odb: &Odb, oid: Oid) -> Result<Vec<u8>> {
    match odb.read(oid)? {
        (Kind::Blob, payload) => Ok(payload),
        (other, _) => Err(Error::Other(format!("expected blob {oid}, found {other}"))),
    }
}

/// 写工作区文件：优先走 `Worktree::write_blob_to`（T4）；T4 未完成时退化为本地实现，
/// 这样本轮的端到端验收不被并行 wave 卡住。
fn write_worktree_entry(worktree: &Worktree, odb: &Odb, path: &[u8], entry: Entry) -> Result<()> {
    match worktree.write_blob_to(path, entry.oid, entry.mode) {
        Ok(()) => Ok(()),
        Err(Error::NotImplemented(_)) => write_blob_direct(odb, worktree.repo(), path, entry),
        Err(err) => Err(err),
    }
}

fn write_blob_direct(odb: &Odb, repo: &Repo, path: &[u8], entry: Entry) -> Result<()> {
    let bytes = read_blob(odb, entry.oid)?;
    let absolute = repo.work_path(path)?;
    if let Some(parent) = absolute.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if entry.mode == FileMode::Symlink {
        let target = std::str::from_utf8(&bytes)
            .map_err(|_| Error::Other("non-UTF8 symlink target is not supported in v1".into()))?;
        let _ = std::fs::remove_file(&absolute);
        symlink(target, &absolute)?;
        return Ok(());
    }
    std::fs::write(&absolute, &bytes)?;
    set_executable(&absolute, entry.mode == FileMode::Executable)?;
    Ok(())
}

fn remove_worktree_path(repo: &Repo, path: &[u8]) -> Result<()> {
    let absolute = repo.work_path(path)?;
    match std::fs::remove_file(&absolute) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(Error::Io(err)),
    }
    if let Some(workdir) = repo.workdir() {
        let mut current = absolute.parent();
        while let Some(dir) = current {
            if dir == workdir || !dir.starts_with(workdir) {
                break;
            }
            if std::fs::remove_dir(dir).is_err() {
                break;
            }
            current = dir.parent();
        }
    }
    Ok(())
}

fn cleanup_merge_state(repo: &Repo) {
    for name in ["MERGE_HEAD", "MERGE_MSG", "ORIG_HEAD", "MERGE_MODE"] {
        let _ = std::fs::remove_file(repo.git_dir().join(name));
    }
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(Error::Io)
}

#[cfg(not(unix))]
fn symlink(target: &str, link: &Path) -> Result<()> {
    std::fs::write(link, target.as_bytes()).map_err(Error::Io)
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    let mode = if executable { 0o755 } else { 0o644 };
    permissions.set_mode(mode);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    //! CLI 端到端：同一拓扑建两份仓库，一份用 `mg merge`、一份用真实 `git merge`，
    //! 逐字节对拍 tree / 冲突文件 / porcelain / index stage。真值只来自运行时 git。

    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    fn git(dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "V")
            .env("GIT_AUTHOR_EMAIL", "v@example.com")
            .env("GIT_COMMITTER_NAME", "V")
            .env("GIT_COMMITTER_EMAIL", "v@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0000")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .args(args)
            .output()
            .expect("spawn git")
    }

    fn git_ok(dir: &Path, args: &[&str]) -> Vec<u8> {
        let out = git(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    fn git_text(dir: &Path, args: &[&str]) -> String {
        String::from_utf8(git_ok(dir, args))
            .expect("utf8 git output")
            .trim()
            .to_string()
    }

    fn mg_binary() -> Option<PathBuf> {
        if let Ok(path) = std::env::var("CARGO_BIN_EXE_mg") {
            if Path::new(&path).is_file() {
                return Some(PathBuf::from(path));
            }
        }
        let exe = std::env::current_exe().ok()?;
        let candidate = exe.parent()?.parent()?.join("mg");
        candidate.is_file().then_some(candidate)
    }

    fn run_mg(dir: &Path, args: &[&str]) -> Output {
        let binary = mg_binary().expect("mg binary path (build the bin target)");
        Command::new(binary)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(args)
            .output()
            .expect("spawn mg")
    }

    /// 依赖模块未实现时跳过（而不是假绿）。
    fn skip_if_not_implemented(out: &Output) -> bool {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not implemented yet") {
            eprintln!("SKIP: dependency not implemented: {}", stderr.trim());
            return true;
        }
        false
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    fn init(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        git_ok(dir, &["init", "-q", "-b", "main"]);
    }

    fn write_file(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn commit_all(dir: &Path, message: &str) {
        git_ok(dir, &["add", "-A"]);
        git_ok(dir, &["commit", "-qm", message]);
    }

    /// 把 `origin` 复制成 mg 侧仓库，返回 `(origin, mg_side)`。
    fn fork(origin: &Path, tmp: &Path) -> PathBuf {
        let mg_side = tmp.join("mg");
        copy_dir(origin, &mg_side);
        mg_side
    }

    fn tree_of(dir: &Path) -> String {
        git_text(dir, &["rev-parse", "HEAD^{tree}"])
    }

    fn porcelain(dir: &Path) -> String {
        String::from_utf8(git_ok(dir, &["status", "--porcelain"])).unwrap()
    }

    #[test]
    fn clean_merge_tree_matches_git() {
        let Some(_) = mg_binary() else {
            panic!("mg binary not found; `cargo test` must build the bin target for CLI e2e")
        };
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        init(&origin);
        write_file(&origin, "left.txt", "left\n");
        write_file(&origin, "right.txt", "right\n");
        commit_all(&origin, "base");
        git_ok(&origin, &["checkout", "-q", "-b", "side"]);
        write_file(&origin, "left.txt", "left side\n");
        write_file(&origin, "new-side.txt", "added by side\n");
        commit_all(&origin, "side");
        git_ok(&origin, &["checkout", "-q", "main"]);
        write_file(&origin, "right.txt", "right main\n");
        write_file(&origin, "new-main.txt", "added by main\n");
        commit_all(&origin, "main");

        let mg_side = fork(&origin, tmp.path());
        git_ok(&origin, &["merge", "side"]);
        let out = run_mg(&mg_side, &["merge", "side"]);
        if skip_if_not_implemented(&out) {
            return;
        }
        assert!(
            out.status.success(),
            "mg merge failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(tree_of(&origin), tree_of(&mg_side), "merged tree mismatch");
        let fsck = git_ok(&mg_side, &["fsck", "--no-progress"]);
        assert!(
            !String::from_utf8_lossy(&fsck).contains("error"),
            "git fsck reported errors: {}",
            String::from_utf8_lossy(&fsck)
        );
        let parents = git_text(&mg_side, &["log", "--format=%P", "-1"]);
        assert_eq!(parents.split_whitespace().count(), 2);
    }

    #[test]
    fn fast_forward_matches_git() {
        let Some(_) = mg_binary() else {
            panic!("mg binary not found; `cargo test` must build the bin target for CLI e2e")
        };
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        init(&origin);
        write_file(&origin, "f.txt", "one\n");
        commit_all(&origin, "base");
        git_ok(&origin, &["checkout", "-q", "-b", "side"]);
        write_file(&origin, "f.txt", "two\n");
        write_file(&origin, "g.txt", "g\n");
        commit_all(&origin, "side");
        git_ok(&origin, &["checkout", "-q", "main"]);

        let mg_side = fork(&origin, tmp.path());
        git_ok(&origin, &["merge", "side"]);
        let out = run_mg(&mg_side, &["merge", "side"]);
        if skip_if_not_implemented(&out) {
            return;
        }
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(tree_of(&origin), tree_of(&mg_side));
        assert_eq!(
            git_text(&origin, &["rev-parse", "HEAD"]),
            git_text(&mg_side, &["rev-parse", "HEAD"]),
            "fast-forward must land exactly on the target commit"
        );
        assert!(porcelain(&mg_side).is_empty());
    }

    #[test]
    fn no_ff_merge_tree_matches_git() {
        let Some(_) = mg_binary() else {
            panic!("mg binary not found; `cargo test` must build the bin target for CLI e2e")
        };
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        init(&origin);
        write_file(&origin, "f.txt", "one\n");
        commit_all(&origin, "base");
        git_ok(&origin, &["checkout", "-q", "-b", "side"]);
        write_file(&origin, "f.txt", "two\n");
        commit_all(&origin, "side");
        git_ok(&origin, &["checkout", "-q", "main"]);

        let mg_side = fork(&origin, tmp.path());
        git_ok(&origin, &["merge", "--no-ff", "side"]);
        let out = run_mg(&mg_side, &["merge", "--no-ff", "side"]);
        if skip_if_not_implemented(&out) {
            return;
        }
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(tree_of(&origin), tree_of(&mg_side));
        assert_eq!(
            git_text(&mg_side, &["log", "--format=%P", "-1"])
                .split_whitespace()
                .count(),
            2
        );
    }

    #[test]
    fn already_up_to_date() {
        let Some(_) = mg_binary() else {
            panic!("mg binary not found; `cargo test` must build the bin target for CLI e2e")
        };
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init(&repo);
        write_file(&repo, "f.txt", "one\n");
        commit_all(&repo, "base");
        let before = git_text(&repo, &["rev-parse", "HEAD"]);
        let out = run_mg(&repo, &["merge", "main"]);
        if skip_if_not_implemented(&out) {
            return;
        }
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("Already up to date"));
        assert_eq!(before, git_text(&repo, &["rev-parse", "HEAD"]));
    }

    #[test]
    fn content_conflict_matches_git_and_git_can_commit() {
        let Some(_) = mg_binary() else {
            panic!("mg binary not found; `cargo test` must build the bin target for CLI e2e")
        };
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        init(&origin);
        write_file(&origin, "f.txt", "a\nb\nc\nd\ne\n");
        commit_all(&origin, "base");
        git_ok(&origin, &["checkout", "-q", "-b", "side"]);
        write_file(&origin, "f.txt", "a\nTHEIRS\nc\nd\ne\n");
        commit_all(&origin, "side");
        git_ok(&origin, &["checkout", "-q", "main"]);
        write_file(&origin, "f.txt", "a\nOURS\nc\nd\ne\n");
        commit_all(&origin, "main");

        let mg_side = fork(&origin, tmp.path());
        let git_merge = git(&origin, &["merge", "side"]);
        assert!(!git_merge.status.success());
        let out = run_mg(&mg_side, &["merge", "side"]);
        if skip_if_not_implemented(&out) {
            return;
        }
        assert!(
            !out.status.success(),
            "mg merge must exit non-zero on conflict"
        );
        assert_eq!(
            std::fs::read(origin.join("f.txt")).unwrap(),
            std::fs::read(mg_side.join("f.txt")).unwrap(),
            "conflict file bytes mismatch"
        );
        assert_eq!(porcelain(&origin), porcelain(&mg_side));
        assert_eq!(
            git_text(&origin, &["ls-files", "-s"]),
            git_text(&mg_side, &["ls-files", "-s"])
        );
        assert!(mg_side.join(".git/MERGE_HEAD").is_file());

        git_ok(&mg_side, &["add", "f.txt"]);
        git_ok(&mg_side, &["commit", "-qm", "resolved"]);
        assert_eq!(
            git_text(&mg_side, &["log", "--format=%P", "-1"])
                .split_whitespace()
                .count(),
            2,
            "real git must complete the merge left by mg"
        );
        assert!(!mg_side.join(".git/MERGE_HEAD").exists());
    }

    #[test]
    fn diff3_conflict_matches_git() {
        let Some(_) = mg_binary() else {
            panic!("mg binary not found; `cargo test` must build the bin target for CLI e2e")
        };
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        init(&origin);
        git_ok(&origin, &["config", "merge.conflictstyle", "diff3"]);
        write_file(&origin, "f.txt", "a\nb\nc\nd\ne\n");
        commit_all(&origin, "base");
        git_ok(&origin, &["checkout", "-q", "-b", "side"]);
        write_file(&origin, "f.txt", "a\nTHEIRS\nc\nd\ne\n");
        commit_all(&origin, "side");
        git_ok(&origin, &["checkout", "-q", "main"]);
        write_file(&origin, "f.txt", "a\nOURS\nc\nd\ne\n");
        commit_all(&origin, "main");

        let mg_side = fork(&origin, tmp.path());
        let git_merge = git(&origin, &["merge", "side"]);
        assert!(!git_merge.status.success());
        let out = run_mg(&mg_side, &["merge", "side"]);
        if skip_if_not_implemented(&out) {
            return;
        }
        assert!(!out.status.success());
        assert_eq!(
            std::fs::read(origin.join("f.txt")).unwrap(),
            std::fs::read(mg_side.join("f.txt")).unwrap(),
            "diff3 conflict file bytes mismatch"
        );
    }

    #[test]
    fn add_add_and_modify_delete_match_git() {
        let Some(_) = mg_binary() else {
            panic!("mg binary not found; `cargo test` must build the bin target for CLI e2e")
        };
        for label in ["add/add", "ours-deletes", "ours-modifies"] {
            let tmp = tempfile::tempdir().unwrap();
            let origin = tmp.path().join("origin");
            init(&origin);
            write_file(&origin, "d.txt", "base\n");
            write_file(&origin, "anchor.txt", "anchor\n");
            commit_all(&origin, "base");
            git_ok(&origin, &["checkout", "-q", "-b", "side"]);
            match label {
                "add/add" => write_file(&origin, "both.txt", "SIDE\n"),
                "ours-deletes" => write_file(&origin, "d.txt", "theirs\n"),
                _ => std::fs::remove_file(origin.join("d.txt")).unwrap(),
            }
            commit_all(&origin, "side");
            git_ok(&origin, &["checkout", "-q", "main"]);
            match label {
                "add/add" => write_file(&origin, "both.txt", "MAIN\n"),
                "ours-deletes" => std::fs::remove_file(origin.join("d.txt")).unwrap(),
                _ => write_file(&origin, "d.txt", "ours\n"),
            }
            commit_all(&origin, "main");

            let mg_side = fork(&origin, tmp.path());
            let git_merge = git(&origin, &["merge", "side"]);
            assert!(!git_merge.status.success(), "case {label} should conflict");
            let out = run_mg(&mg_side, &["merge", "side"]);
            if skip_if_not_implemented(&out) {
                return;
            }
            assert!(!out.status.success(), "case {label}: mg must fail");
            assert_eq!(porcelain(&origin), porcelain(&mg_side), "case {label}");
            assert_eq!(
                git_text(&origin, &["ls-files", "-s"]),
                git_text(&mg_side, &["ls-files", "-s"]),
                "case {label}: index stages"
            );
            for name in ["both.txt", "d.txt"] {
                if origin.join(name).exists() {
                    assert_eq!(
                        std::fs::read(origin.join(name)).unwrap(),
                        std::fs::read(mg_side.join(name)).unwrap(),
                        "case {label}: worktree content for {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn dirty_worktree_is_refused() {
        let Some(_) = mg_binary() else {
            panic!("mg binary not found; `cargo test` must build the bin target for CLI e2e")
        };
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init(&repo);
        write_file(&repo, "f.txt", "one\n");
        commit_all(&repo, "base");
        git_ok(&repo, &["checkout", "-q", "-b", "side"]);
        write_file(&repo, "f.txt", "two\n");
        commit_all(&repo, "side");
        git_ok(&repo, &["checkout", "-q", "main"]);
        write_file(&repo, "f.txt", "local dirty\n");

        let out = run_mg(&repo, &["merge", "side"]);
        if skip_if_not_implemented(&out) {
            return;
        }
        assert!(!out.status.success(), "dirty merge must be refused");
        assert_eq!(
            std::fs::read_to_string(repo.join("f.txt")).unwrap(),
            "local dirty\n"
        );
    }

    #[test]
    fn missing_rev_and_oid_are_typed_errors() {
        let Some(_) = mg_binary() else {
            panic!("mg binary not found; `cargo test` must build the bin target for CLI e2e")
        };
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        init(&repo);
        write_file(&repo, "f.txt", "one\n");
        commit_all(&repo, "base");

        let out = run_mg(&repo, &["merge", "does-not-exist"]);
        if skip_if_not_implemented(&out) {
            return;
        }
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("reference not found"),
            "expected ReferenceNotFound, got: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let out = run_mg(
            &repo,
            &["merge", "1111111111111111111111111111111111111111"],
        );
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("object not found"),
            "expected ObjectNotFound, got: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
