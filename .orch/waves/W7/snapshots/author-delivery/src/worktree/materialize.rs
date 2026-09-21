//! tree → 工作区物化（checkout / switch / reset --hard）。**T11（omp）实现范围**。
//!
//! # 冻结语义（任务书 §3.1）
//!
//! * **先删后写**：index 里有、目标 tree 里没有的**已跟踪**路径从工作区删掉，
//!   删完顺手清掉变空的目录。**未跟踪与 ignored 文件一律不动。**
//! * 目录先建后写；`100755` 显式 `chmod 0755`；`120000` 写 symlink；`40000` 是子树（递归读 odb）。
//! * `force == false` 时**先全量校验、再动磁盘**：任何会被覆盖的本地改动都返回
//!   `Error::WouldLoseChanges`，失败后工作区与 index 一个字节都不变。
//! * 绝不跟随 symlink：**删除**（`unlink`）与**剪枝**（`rmdir`）之前逐段确认祖先不是 symlink，
//!   有一段不是真目录就整条路径都不碰（否则会顺手删掉工作区之外的**数据**）；
//!   文件/symlink 一律「写临时名 → `rename`」，因此永远不会跟随目标路径上的 symlink。
//! * 路径是**字节**，非 UTF-8 照样写（`Repo::work_path` 只收 UTF-8，这里自己拼 `OsStr`）。
//!
//! # `?` 的语义
//!
//! 四条命令都要「把 tree 展平」，而 `cli/mod.rs` 是 CONTROLLER-OWNED、不能新增模块文件，
//! 所以**本文件**是 T11 的共享辅助之家（任务书 §2 明确允许在这里新增 `flatten_tree`）。
//!
//! # 与真实 git 的对齐（git 2.55 实测，见交付说明里的 e2e 对拍）
//!
//! 更新集合的定义（这是「改动不会被覆盖的文件保留不动」的关键）：
//! `force == false` 时**只有 index 与目标 tree 不一致的路径**会被写；index 与目标一致的路径
//! 原样不动 —— 于是「本地改了、但两个分支里内容相同」的文件会连内容带改动一起留下（git 同）；
//! `force == true`（`reset --hard` / `checkout -f`）则重写目标 tree 的每个路径。
//!
//! | 情形 | 真实 git | 本实现 |
//! |---|---|---|
//! | 目标路径在工作区有改动（与 index 的 oid/mode 不一致） | 拒绝（`Your local changes … would be overwritten`） | `WouldLoseChanges` |
//! | 该路径在两边的 tree 里相同 | 不动工作区，保留改动 | 不在更新集合里，不动 |
//! | 该路径在工作区被删掉了 | 允许，直接写出目标内容 | 缺失不算「有改动」 |
//! | 目标要删掉一个本地改过的文件 | 拒绝 | `WouldLoseChanges` |
//! | 工作区有未跟踪文件挡住目标路径 | 拒绝（`untracked working tree files would be overwritten`） | `WouldLoseChanges` |
//! | 挡路的未跟踪文件被 ignore | 静默覆盖 | 覆盖 |
//! | 挡路的空目录 | `rmdir` 后写 | `rmdir` 后写 |
//! | 挡路的非空目录（内容被 ignore） | 直接删掉 | 删掉 |
//! | 挡路的非空目录（内容未被 ignore） | 拒绝（`would lose untracked files`） | `WouldLoseChanges` |
//! | 写出路径的祖先被普通文件挡住 | 拒绝（`-f` 时删掉它换真目录） | 同 |
//! | 写出路径的祖先是指向工作区外的 symlink，顺着它 stat 不到目标 | 把 symlink 换成真目录后写出（工作区外零改动） | 同 |
//! | 写出路径的祖先是指向工作区外的 symlink，顺着它 stat 得到目标 | 拒绝（`Your local changes … would be overwritten`） | `WouldLoseChanges` |
//! | 删除路径的祖先被 symlink / 普通文件挡住，顺着它 stat 得到东西 | 拒绝（同上；与内容是否相同无关） | `WouldLoseChanges` |
//! | 删除路径的祖先被 symlink / 普通文件挡住，顺着它 stat 不到东西 | 放行（磁盘上本来就没有可删之物，symlink 原样留下） | 同 |
//! | `-f` 下删除路径的祖先被 symlink / 普通文件挡住 | 跳过该路径，工作区外零改动、symlink 原样留下 | 同 |
//! | 祖先是被跟踪、且本次会被删掉的 symlink | 删掉 symlink 再建目录 | 同 |

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry, StatData, TREE_EXTENSION};
use crate::object::{FileMode, Kind, Object, Tree};
use crate::odb::Odb;
use crate::oid::Oid;
use crate::refs::{Head, RefStore, HEADS_PREFIX};
use crate::repo::Repo;
use crate::worktree::Ignore;

/// `100644` 落盘用的权限位。
const REGULAR_BITS: u32 = 0o644;
/// `100755` 落盘用的权限位（显式 `chmod`，不靠 umask）。
const EXECUTABLE_BITS: u32 = 0o755;
/// 临时文件名后缀：先写 `NAME.mg-tmp` 再 `rename`，既有原子性又不跟随 symlink。
const TEMP_SUFFIX: &str = ".mg-tmp";

#[derive(Debug, Clone, Copy, Default)]
pub struct MaterializeOptions {
    /// true = 覆盖本地改动（`reset --hard` / `checkout -f`）。
    pub force: bool,
}

// ---------------------------------------------------------------- 展平 tree

/// 展平 tree：`(仓库相对全路径, oid, mode)`，按路径**字节升序**；`FileMode::Tree` 递归展开。
///
/// 本文件唯一新增的公共符号（任务书 §2）：`switch` / `checkout` / `reset` 都要展平 tree，
/// 而 `cli/mod.rs` 被冻结、不能新增模块文件。
pub fn flatten_tree(odb: &Odb, tree: Oid) -> Result<Vec<(Vec<u8>, Oid, FileMode)>> {
    let mut out = Vec::new();
    let object = odb.read_object(tree)?;
    let tree = object.into_tree()?;
    flatten_into(odb, &tree, &mut Vec::new(), &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// 从已经是 `Tree` 的对象出发展平（`materialize_tree` 拿到的是 `&Tree`，没有 oid）。
fn flatten_into(
    odb: &Odb,
    tree: &Tree,
    prefix: &mut Vec<u8>,
    out: &mut Vec<(Vec<u8>, Oid, FileMode)>,
) -> Result<()> {
    for entry in tree.entries() {
        validate_name(&entry.name)?;
        let mark = prefix.len();
        if mark > 0 {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(&entry.name);
        if entry.mode.is_tree() {
            let subtree = odb.read_object(entry.oid)?.into_tree()?;
            flatten_into(odb, &subtree, prefix, out)?;
        } else {
            out.push((prefix.clone(), entry.oid, entry.mode));
        }
        prefix.truncate(mark);
    }
    Ok(())
}

fn validate_name(name: &[u8]) -> Result<()> {
    if name.is_empty() || name.contains(&b'/') || name.contains(&0) || name == b"." || name == b".."
    {
        return Err(Error::corrupt(
            "tree entry name",
            format!("{:?}", String::from_utf8_lossy(name)),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- 写单个 blob

/// 按 mode 把某个 blob 写到工作区：普通文件 / 可执行位 / symlink。
///
/// * 内容先写同目录临时名再 `rename`：即使目标路径是个 symlink 也只是**替换**它，
///   不会顺着它写到工作区之外（`Repo::work_path` 只做了一部分检查，这里再确认一遍）。
/// * 祖先里出现 symlink 或普通文件挡住目录 → 拒绝（不跟随、不覆盖）。
pub fn write_blob_to(repo: &Repo, path: &[u8], oid: Oid, mode: FileMode) -> Result<()> {
    let workdir = workdir(repo)?;
    let segments = split_rel_path(path)?;
    let parents = segments
        .split_last()
        .map(|(_, parents)| parents)
        .ok_or_else(|| Error::corrupt("path", "empty path".to_string()))?;
    ensure_parents(workdir, parents)?;

    if mode.is_tree() {
        return Err(Error::Other(format!(
            "refusing to write a tree entry to the working tree: {}",
            display_path(path)
        )));
    }
    let (kind, payload) = Odb::new(repo).read(oid)?;
    if kind != Kind::Blob {
        return Err(Error::corrupt(
            "working tree write",
            format!("{} is a {}, expected a blob", oid.to_hex(), kind),
        ));
    }
    let target = worktree_path(repo, path)?;
    clear_blocking_dir(&target)?;

    match mode {
        FileMode::Symlink => atomic_write(&target, |tmp| {
            std::os::unix::fs::symlink(OsStr::from_bytes(&payload), tmp)?;
            Ok(())
        }),
        FileMode::Regular | FileMode::Executable => {
            let bits = if mode == FileMode::Executable {
                EXECUTABLE_BITS
            } else {
                REGULAR_BITS
            };
            atomic_write(&target, |tmp| {
                fs::write(tmp, &payload)?;
                fs::set_permissions(tmp, fs::Permissions::from_mode(bits))?;
                Ok(())
            })
        }
        FileMode::Tree => unreachable!("handled above"),
    }
}

/// 写临时文件 + `rename`；任何一步失败都把临时文件清掉。
fn atomic_write<F>(target: &Path, create: F) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let tmp = temp_path(target);
    let _ = fs::remove_file(&tmp);
    if let Err(err) = create(&tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    match fs::rename(&tmp, target) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err.into())
        }
    }
}

fn temp_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(OsStr::to_os_string)
        .unwrap_or_else(|| OsStr::new("mg").to_os_string());
    name.push(TEMP_SUFFIX);
    target.with_file_name(name)
}

/// 逐段确保父目录存在；任何一段是 symlink 或普通文件 → 拒绝（保证不越出工作区）。
fn ensure_parents(workdir: &Path, parents: &[&[u8]]) -> Result<()> {
    let mut current = workdir.to_path_buf();
    for segment in parents {
        current.push(OsStr::from_bytes(segment));
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::WouldLoseChanges(format!(
                    "refusing to write through symlink: {}",
                    current.display()
                )))
            }
            Ok(_) => {
                return Err(Error::WouldLoseChanges(format!(
                    "not a directory: {}",
                    current.display()
                )))
            }
            Err(err) if err.kind() == ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

/// 目标路径是个目录时先清掉：空的直接 `rmdir`，非空的（未经校验就到了这里）拒绝。
fn clear_blocking_dir(target: &Path) -> Result<()> {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_dir() => {
            if dir_has_entries(target)? {
                return Err(Error::WouldLoseChanges(format!(
                    "{} is a non-empty directory",
                    target.display()
                )));
            }
            fs::remove_dir(target)?;
            Ok(())
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------- 物化整棵树

/// 物化计划：要写哪些路径、要删哪些路径（都按路径升序）。
struct Plan<'a> {
    writes: Vec<&'a (Vec<u8>, Oid, FileMode)>,
    removals: Vec<Vec<u8>>,
}

/// 一次物化的结果。`materialize_tree` 只把 `removed` 暴露出去（冻结签名）；
/// 上层命令还需要 `written` 来决定重建 index 时哪些路径取新 stat。
pub(crate) struct Applied {
    /// 本次真正从工作区删掉的路径（升序）。
    pub removed: Vec<Vec<u8>>,
    /// 本次真正写出的路径（升序）。
    pub written: Vec<Vec<u8>>,
}

/// 把 tree 物化到工作区，返回**本次从工作区删除的、仓库相对路径列表**（升序）。
///
/// 「删除」只指「index 里有、目标 tree 里没有」的已跟踪路径真的从磁盘上消失；
/// 未跟踪与 ignored 文件不会被删，目录只有在搬空后才会被清掉。
pub fn materialize_tree(
    repo: &Repo,
    tree: &Tree,
    opts: &MaterializeOptions,
) -> Result<Vec<Vec<u8>>> {
    let odb = Odb::new(repo);
    let mut target = Vec::new();
    flatten_into(&odb, tree, &mut Vec::new(), &mut target)?;
    target.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(apply_tree(repo, &target, opts.force)?.removed)
}

/// 校验 + 物化（`materialize_tree` 与四条命令共用）。`entries` 必须是升序的展平 tree。
pub(crate) fn apply_tree(
    repo: &Repo,
    target: &[(Vec<u8>, Oid, FileMode)],
    force: bool,
) -> Result<Applied> {
    let ignore = Ignore::load(repo)?;
    let previous = Index::read(repo)?;
    let plan = plan(repo, &previous, target, force, &ignore)?;
    apply_plan(repo, &plan, force)
}

/// 只做校验不动物化（`switch -c` 必须先确认不会被拒绝，再创建分支引用）。
pub(crate) fn check_tree(
    repo: &Repo,
    target: &[(Vec<u8>, Oid, FileMode)],
    force: bool,
) -> Result<()> {
    let ignore = Ignore::load(repo)?;
    let previous = Index::read(repo)?;
    plan(repo, &previous, target, force, &ignore).map(|_| ())
}

/// index 里 stage 0 条目的快查表。
///
/// `Index::lookup` 是线性扫描，逐路径调用会退化成 O(路径数 × 条目数)；
/// 物化一棵大树时这个差别很实在，所以本文件统一走这张表。
fn stage0_map(index: &Index) -> BTreeMap<&[u8], &IndexEntry> {
    index
        .entries
        .iter()
        .filter(|entry| entry.stage == 0)
        .map(|entry| (entry.path.as_slice(), entry))
        .collect()
}

fn plan<'a>(
    repo: &Repo,
    index: &Index,
    target: &'a [(Vec<u8>, Oid, FileMode)],
    force: bool,
    ignore: &Ignore,
) -> Result<Plan<'a>> {
    let current = stage0_map(index);
    let mut target_map: BTreeMap<&[u8], (Oid, FileMode)> = BTreeMap::new();
    for entry in target {
        target_map.insert(entry.0.as_slice(), (entry.1, entry.2));
    }
    // 已跟踪 = index 里出现过的所有路径（含冲突的 stage 1/2/3）。
    let tracked: BTreeSet<&[u8]> = index
        .entries
        .iter()
        .map(|entry| entry.path.as_slice())
        .collect();

    let mut writes = Vec::new();
    for entry in target {
        let stale = match current.get(entry.0.as_slice()) {
            Some(existing) => existing.oid != entry.1 || existing.mode != entry.2,
            None => true,
        };
        if force || stale {
            writes.push(entry);
        }
    }
    let removals: Vec<Vec<u8>> = tracked
        .iter()
        .filter(|path| !target_map.contains_key(*path))
        .map(|path| path.to_vec())
        .collect();

    if !force {
        validate(repo, index, &writes, &removals, &tracked, ignore)?;
    }
    Ok(Plan { writes, removals })
}

/// 全量校验：任何会被覆盖/丢掉的本地内容都返回 `WouldLoseChanges`。
///
/// 祖先里出现 symlink / 普通文件（不只是最后一段）时，规则来自真实 git 2.55 实测：
/// **顺着它 `stat` 得到东西 → 拒绝**（与内容是否逐字节相同无关）；**顺着它 `stat` 不到 → 放行**，
/// 因为文件系统上本来就没有可删 / 可覆盖之物（详见模块头部的对照表）。
fn validate(
    repo: &Repo,
    index: &Index,
    writes: &[&(Vec<u8>, Oid, FileMode)],
    removals: &[Vec<u8>],
    tracked: &BTreeSet<&[u8]>,
    ignore: &Ignore,
) -> Result<()> {
    let current = stage0_map(index);
    let removals_set: BTreeSet<&[u8]> = removals.iter().map(Vec::as_slice).collect();

    // (1) 会被写的路径。
    for entry in writes {
        let path = entry.0.as_slice();
        let Some(barrier) = ancestor_barrier(repo, path)? else {
            // 祖先都是真目录（或整条都不存在）：按内容判定本地改动，与 git 的 `verify_uptodate` 同。
            if let Some(existing) = current.get(path) {
                if dirty_against_index(repo, path, existing, ignore)? {
                    return Err(Error::WouldLoseChanges(display_path(path)));
                }
            }
            // 未跟踪的东西挡住写路径（git: "untracked working tree files would be overwritten"）。
            if !current.contains_key(path) {
                if let Some(blocker) = untracked_blocker(repo, path, ignore)? {
                    return Err(Error::WouldLoseChanges(display_path(&blocker)));
                }
            }
            continue;
        };
        // 祖先不是真目录。放行只有两种情形，别的都拒绝：
        // * 该祖先被本次物化删掉（已跟踪且不在目标 tree 里）→ git 先删它，再建真目录；
        // * symlink 祖先 + 目标路径已跟踪 + 顺着它 stat 不到目标 →
        //   git 把 symlink 换成真目录后写出（实测 `git switch` exit 0，工作区外零改动）。
        let removed_ancestor = tracked.contains(barrier.prefix.as_slice())
            && removals_set.contains(barrier.prefix.as_slice());
        let replaced_symlink = barrier.symlink
            && current.contains_key(path)
            && !path_exists(&worktree_path(repo, path)?)?;
        if !removed_ancestor && !replaced_symlink {
            return Err(Error::WouldLoseChanges(display_path(&barrier.prefix)));
        }
    }
    // (2) 会被删的已跟踪路径：同样不允许删掉本地改动。
    for path in removals {
        let Some(barrier) = ancestor_barrier(repo, path)? else {
            if let Some(existing) = current.get(path.as_slice()) {
                if dirty_against_index(repo, path, existing, ignore)? {
                    return Err(Error::WouldLoseChanges(display_path(path)));
                }
            }
            continue;
        };
        // 普通文件挡住的祖先连「stat 不到」都不放行（git 实测同样报 `local changes`）。
        if !barrier.symlink || path_exists(&worktree_path(repo, path)?)? {
            return Err(Error::WouldLoseChanges(display_path(path)));
        }
    }
    Ok(())
}

/// 工作区在该路径上是否「与 index 不一致」。
///
/// * 文件不存在 → 不算（git 允许：`E36` 里本地删掉的文件会被 checkout 写回来）。
/// * 目录：空的算一致（git 会 `rmdir` 后写，`E12b`）；非空且未被 ignore 才算不一致。
/// * 文件/symlink：比对内容 oid 与 mode（racy-git 不信任 stat，始终真读一次）。
fn dirty_against_index(
    repo: &Repo,
    path: &[u8],
    entry: &IndexEntry,
    ignore: &Ignore,
) -> Result<bool> {
    let fs_path = worktree_path(repo, path)?;
    let meta = match fs::symlink_metadata(&fs_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_dir() {
        return Ok(dir_has_entries(&fs_path)? && !ignore.is_ignored(path, true));
    }
    Ok(worktree_oid_mode(&fs_path)? != (entry.oid, entry.mode))
}

/// 未跟踪的东西擋住写路径时的判定：`Some(path)` = 会被覆盖的路径。
fn untracked_blocker(repo: &Repo, path: &[u8], ignore: &Ignore) -> Result<Option<Vec<u8>>> {
    let fs_path = worktree_path(repo, path)?;
    let meta = match fs::symlink_metadata(&fs_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let is_dir = meta.file_type().is_dir();
    if !is_dir {
        return Ok(if ignore.is_ignored(path, false) {
            None
        } else {
            Some(path.to_vec())
        });
    }
    if !dir_has_entries(&fs_path)? {
        // 空目录：git 直接 rmdir 后写，不算「挡住」。
        return Ok(None);
    }
    Ok(if ignore.is_ignored(path, true) {
        None
    } else {
        Some(path.to_vec())
    })
}

/// 写/删路径上第一个「不是真目录」的祖先段。
struct AncestorBarrier {
    /// 该祖先的仓库相对路径。
    prefix: Vec<u8>,
    /// `true` = symlink（写出时会被换成真目录），`false` = 普通文件。
    symlink: bool,
}

/// 从工作区根开始逐段检查 `path` 的祖先，返回第一个不是真目录的段。
///
/// * 全是真目录 → `None`；某一段不存在 → `None`（再深的路径在文件系统上不可能存在）。
/// * symlink / 普通文件 → `Some(..)`：**绝不能**顺着它继续 `stat` / `unlink` / `rmdir`，
///   否则会操作到工作区之外（真实 git 同样不跟随 symlink 祖先）。
fn ancestor_barrier(repo: &Repo, path: &[u8]) -> Result<Option<AncestorBarrier>> {
    let segments = split_rel_path(path)?;
    let mut prefix = Vec::new();
    for segment in &segments[..segments.len() - 1] {
        let mark = prefix.len();
        if mark > 0 {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(segment);
        match fs::symlink_metadata(worktree_path(repo, &prefix)?) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(meta) => {
                return Ok(Some(AncestorBarrier {
                    prefix,
                    symlink: meta.file_type().is_symlink(),
                }))
            }
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        }
        prefix.truncate(mark);
    }
    Ok(None)
}

/// 目标路径在文件系统上是否 `stat` 得到（会顺着 symlink 祖先往下走）。
///
/// `ENOENT` / `ENOTDIR` 都算「什么都没有」—— 两者都意味着该路径上无物可删、可覆盖。
fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Ok(false)
        }
        Err(err) => Err(err.into()),
    }
}

fn apply_plan(repo: &Repo, plan: &Plan<'_>, force: bool) -> Result<Applied> {
    let mut removed = Vec::new();
    for path in &plan.removals {
        // 文件已经不在工作区时不用删，但空掉的目录仍要清（git 的 checkout/reset 都会 prune）。
        if remove_worktree_entry(repo, path)? {
            removed.push(path.clone());
        }
        prune_empty_dirs(repo, path)?;
    }
    let mut written = Vec::new();
    for entry in &plan.writes {
        prepare_write_target(repo, &entry.0, force)?;
        write_blob_to(repo, &entry.0, entry.1, entry.2)?;
        written.push(entry.0.clone());
    }
    Ok(Applied { removed, written })
}

/// 删掉工作区里的一个条目；目录不动（未跟踪内容不归 checkout 管）。返回是否真的删了东西。
///
/// 祖先里只要有一段不是真目录（symlink / 普通文件 / 缺失）就什么都不做：
/// 顺着 symlink 祖先 `unlink` 会删掉**工作区之外**的数据（真实 git 同样不跟随）。
/// 跨文件的批量删除里，一个路径被跳过不影响其它路径。
fn remove_worktree_entry(repo: &Repo, path: &[u8]) -> Result<bool> {
    if ancestor_barrier(repo, path)?.is_some() {
        return Ok(false);
    }
    let fs_path = worktree_path(repo, path)?;
    match fs::symlink_metadata(&fs_path) {
        Ok(meta) if meta.file_type().is_dir() => Ok(false),
        Ok(_) => {
            fs::remove_file(&fs_path)?;
            Ok(true)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// 删完文件后把空掉的祖先目录一层层清掉（git 的 checkout 也会 prune），停在首个非空目录。
///
/// 每一层 `rmdir` 之前都要确认该目录的祖先里没有 symlink / 普通文件：
/// `rmdir("work/a/b")` 在 `work/a` 是 symlink 时会删掉**工作区之外**的 `b`。
fn prune_empty_dirs(repo: &Repo, path: &[u8]) -> Result<()> {
    let segments = split_rel_path(path)?;
    for depth in (1..segments.len()).rev() {
        let mut rel = Vec::new();
        for (index, segment) in segments[..depth].iter().enumerate() {
            if index > 0 {
                rel.push(b'/');
            }
            rel.extend_from_slice(segment);
        }
        // 障碍段以上的层一律不碰；缺失段交给下面的 `remove_dir` 失败兜住。
        if ancestor_barrier(repo, &rel)?.is_some() {
            break;
        }
        if fs::remove_dir(worktree_path(repo, &rel)?).is_err() {
            break;
        }
    }
    Ok(())
}

/// 写出前的清场：祖先里的 symlink（换成真目录）、普通文件（只有 `force` 时才删）、挡路的目录。
///
/// `checkout <rev> -- <path>` 也用这个：那一路径形态本来就有意覆盖本地改动。
pub(crate) fn prepare_write_target(repo: &Repo, path: &[u8], force: bool) -> Result<()> {
    let segments = split_rel_path(path)?;
    let mut prefix = Vec::new();
    for segment in &segments[..segments.len() - 1] {
        let mark = prefix.len();
        if mark > 0 {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(segment);
        let fs_path = worktree_path(repo, &prefix)?;
        let barrier = match fs::symlink_metadata(&fs_path) {
            Ok(meta) if meta.file_type().is_dir() => None,
            Ok(meta) => Some(meta.file_type().is_symlink()),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        if let Some(symlink) = barrier {
            // symlink 祖先：真实 git 直接把它换成真目录（实测 `git switch` 在这种状态下
            // exit 0，且工作区外零改动），这里照做 —— 校验已确认过这条路径可以写。
            // 普通文件只有 `-f` 才删（非 force 时 git 拒绝，`validate` 也拦住了）。
            if !force && !symlink {
                return Err(Error::WouldLoseChanges(display_path(&prefix)));
            }
            fs::remove_file(&fs_path)?;
            prune_empty_dirs(repo, &prefix)?;
            // 真目录紧接着由 `write_blob_to` → `ensure_parents` 建出来，内容因此落在工作区内。
        }
        prefix.truncate(mark);
    }

    let target = worktree_path(repo, path)?;
    let is_dir = fs::symlink_metadata(&target)
        .map(|meta| meta.file_type().is_dir())
        .unwrap_or(false);
    if is_dir {
        if force {
            fs::remove_dir_all(&target)?;
        } else {
            clear_blocking_dir(&target)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- T11 共享上层辅助

/// 切换目标：附加到某个分支，或 detached 到某个提交。
pub(crate) enum HeadTarget<'a> {
    Attached(&'a str),
    Detached(Oid),
}

/// 工作区绝对路径（按**字节**拼，非 UTF-8 路径也能落地；`Repo::work_path` 只收 UTF-8）。
pub(crate) fn worktree_path(repo: &Repo, rel: &[u8]) -> Result<PathBuf> {
    let workdir = workdir(repo)?;
    let mut path = workdir.to_path_buf();
    for segment in split_rel_path(rel)? {
        path.push(OsStr::from_bytes(segment));
    }
    Ok(path)
}

/// 解析 rev → 提交 oid（附注 tag 会剥一层）。复用 `RefStore::resolve` 的 v1 形式。
pub(crate) fn resolve_commit(repo: &Repo, rev: &str) -> Result<Oid> {
    let oid = RefStore::new(repo).resolve(rev)?;
    peel_commit(&Odb::new(repo), oid)
}

/// `HEAD` 指向的提交；仓库还没有提交时为 `None`。
pub(crate) fn head_commit(repo: &Repo) -> Result<Option<Oid>> {
    match RefStore::new(repo).resolve("HEAD") {
        Ok(oid) => Ok(Some(peel_commit(&Odb::new(repo), oid)?)),
        Err(Error::RefNotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

/// 提交对象里的 tree oid。
pub(crate) fn commit_tree(repo: &Repo, commit: Oid) -> Result<Oid> {
    let object = Odb::new(repo).read_object(commit)?;
    Ok(object.into_commit()?.tree)
}

/// `flatten_tree` 的便捷包装（每次开一个 `Odb`）。
pub(crate) fn tree_entries(repo: &Repo, tree: Oid) -> Result<Vec<(Vec<u8>, Oid, FileMode)>> {
    flatten_tree(&Odb::new(repo), tree)
}

/// 用目标 tree 重建 index。
///
/// * 与旧 index 条目 `(oid, mode)` 完全相同的路径：**原样保留**（含 stat 与 flags）——
///   这样「本地改了内容但两个 tree 相同」的文件在新 index 里仍然看得出是 ` M`（git 同）。
/// * 本次**写出**的路径：stat 取落盘后的 metadata（`git reset --hard` 的形态）。
/// * 其余改动过的路径：stat 清零（`git reset --mixed` 的形态：git 会重新哈希内容，因此
///   不会把「工作区改了但 index 也换了 oid」误判成干净）。
/// * TREE（cache-tree）缓存一律丢掉：条目变了，缓存必然失效。
pub(crate) fn tree_index(
    repo: &Repo,
    entries: &[(Vec<u8>, Oid, FileMode)],
    previous: &Index,
    written: Option<&BTreeSet<Vec<u8>>>,
) -> Result<Index> {
    let mut index = Index {
        version: 2,
        entries: Vec::with_capacity(entries.len()),
        tree_oid: None,
        extensions: previous
            .extensions
            .iter()
            .filter(|ext| ext.signature != TREE_EXTENSION)
            .cloned()
            .collect(),
    };
    let current = stage0_map(previous);
    for (path, oid, mode) in entries {
        let mut entry = IndexEntry::new(path.clone(), *oid, *mode);
        match current.get(path.as_slice()) {
            Some(old) if old.oid == *oid && old.mode == *mode => {
                entry.stat = old.stat;
                entry.assume_valid = old.assume_valid;
                entry.extended = old.extended;
                entry.flags_raw = old.flags_raw;
            }
            _ if written.is_some_and(|set| set.contains(path)) => {
                entry.stat =
                    StatData::from_metadata(&fs::symlink_metadata(worktree_path(repo, path)?)?);
            }
            _ => {}
        }
        index.entries.push(entry);
    }
    index.sort();
    Ok(index)
}

/// 切到某个提交：物化工作区 → 重建 index → 更新 HEAD。
///
/// 顺序与守卫都按真实 git 实测对齐：
/// * index 里还有未解决的冲突（stage 1/2/3）且没有 `-f` → 拒绝
///   （git: `you need to resolve your current index first`；加了 `-f` 则照做并清掉冲突）。
/// * 目标提交 == 当前 HEAD 提交且没有 `-f` → **只改 HEAD**：git 此时不碰 index 与工作区
///   （实测 P1/P2/P3/P8/P9/P10，暂存改动 / 本地删除 / 未跟踪文件都原样保留；
///   index 文件本身不存在的病态状态除外 —— 那时 git 走 unpack_trees 并可能报错，见 P7）。
/// * 其余情况：物化（含全量校验）→ 重建 index → 改 HEAD；被拒绝时 HEAD 与 index 都不动过。
pub(crate) fn checkout_to(
    repo: &Repo,
    commit: Oid,
    force: bool,
    head: HeadTarget<'_>,
) -> Result<()> {
    let previous = Index::read(repo)?;
    if !force && previous.has_conflicts() {
        return Err(Error::Other(
            "you need to resolve your current index first".to_string(),
        ));
    }
    if same_commit_switch(repo, commit, force, &previous)? {
        return set_head(repo, head);
    }

    let entries = tree_entries(repo, commit_tree(repo, commit)?)?;
    let applied = apply_tree(repo, &entries, force)?;
    let written: BTreeSet<Vec<u8>> = applied.written.into_iter().collect();
    let index = tree_index(repo, &entries, &previous, Some(&written))?;
    index.write(repo)?;
    set_head(repo, head)
}

/// `checkout_to` 是否会走「只改 HEAD」的捷径（`switch -c` 的预校验也要问同一个问题）。
pub(crate) fn same_commit_switch(
    repo: &Repo,
    commit: Oid,
    force: bool,
    previous: &Index,
) -> Result<bool> {
    if force || previous.version == 0 {
        // version == 0 = index 文件不存在（`dirc::read_index` 的约定）：git 那时不走捷径。
        return Ok(false);
    }
    Ok(head_commit(repo)? == Some(commit))
}

fn set_head(repo: &Repo, head: HeadTarget<'_>) -> Result<()> {
    let refs = RefStore::new(repo);
    match head {
        HeadTarget::Attached(branch) => refs.set_head_attached(branch),
        HeadTarget::Detached(oid) => refs.set_head_detached(oid),
    }
}

/// 合并进行中（`.git/MERGE_HEAD` 存在）时 `git switch` 会拒绝（git: `cannot switch branch while merging`）。
pub(crate) fn merge_in_progress(repo: &Repo) -> bool {
    repo.git_dir().join("MERGE_HEAD").exists()
}

/// 分支全名。
pub(crate) fn full_ref(branch: &str) -> String {
    format!("{HEADS_PREFIX}{branch}")
}

/// 当前分支的短名（detached 或 HEAD 缺失时为 `None`）。
pub(crate) fn current_branch(repo: &Repo) -> Result<Option<String>> {
    match RefStore::new(repo).read_head() {
        Ok(Head::Attached(name)) => {
            let short = name.strip_prefix(HEADS_PREFIX).unwrap_or(name.as_str());
            Ok(Some(short.to_string()))
        }
        Ok(Head::Detached(_)) => Ok(None),
        Err(Error::RefNotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

/// 分支引用当前指向的 oid；不存在返回 `None`。
pub(crate) fn branch_oid(repo: &Repo, branch: &str) -> Result<Option<Oid>> {
    match RefStore::new(repo).resolve(&full_ref(branch)) {
        Ok(oid) => Ok(Some(oid)),
        Err(Error::RefNotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

// ---------------------------------------------------------------- 小工具

fn workdir(repo: &Repo) -> Result<&Path> {
    repo.workdir()
        .ok_or(Error::Unsupported("bare repository has no working tree"))
}

/// 把仓库相对路径切成段，并拒绝空段 / `.` / `..` / NUL（与 `Repo::work_path` 同规则）。
fn split_rel_path(rel: &[u8]) -> Result<Vec<&[u8]>> {
    if rel.is_empty() {
        return Err(Error::corrupt("path", "empty path".to_string()));
    }
    let mut out = Vec::new();
    for segment in rel.split(|byte| *byte == b'/') {
        if segment.is_empty() || segment == b"." || segment == b".." || segment.contains(&0) {
            return Err(Error::Other(format!(
                "unsafe path in tree/index: {}",
                display_path(rel)
            )));
        }
        out.push(segment);
    }
    Ok(out)
}

/// 工作区里该文件/符号链接的 `(内容 oid, mode)`。
fn worktree_oid_mode(path: &Path) -> Result<(Oid, FileMode)> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(path)?;
        return Ok((
            Oid::hash_object("blob", target.as_os_str().as_bytes()),
            FileMode::Symlink,
        ));
    }
    let bytes = fs::read(path)?;
    let mode = if meta.permissions().mode() & 0o111 != 0 {
        FileMode::Executable
    } else {
        FileMode::Regular
    };
    Ok((Oid::hash_object("blob", &bytes), mode))
}

fn dir_has_entries(path: &Path) -> Result<bool> {
    let mut entries = fs::read_dir(path)?;
    Ok(entries.next().is_some())
}

fn display_path(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

/// 剥掉附注 tag 的一层（`mg cat-file`/`diff` 的处理方式，保持全项目一致）。
fn peel_commit(odb: &Odb, oid: Oid) -> Result<Oid> {
    match odb.read_object(oid)? {
        Object::Commit(_) => Ok(oid),
        Object::Tag(tag) => {
            let object = odb.read_object(tag.object)?;
            match object {
                Object::Commit(_) => Ok(tag.object),
                other => Err(Error::Other(format!(
                    "expected commit, got {} (from tag {})",
                    other.kind(),
                    oid.to_hex()
                ))),
            }
        }
        other => Err(Error::Other(format!(
            "expected commit, got {}",
            other.kind()
        ))),
    }
}

#[cfg(test)]
mod tests {
    //! 真值一律来自**真实 git 进程**与文件系统：`git ls-tree` / `ls-files --stage` /
    //! `status --porcelain` 现场取，不硬编码任何期望字节。

    use super::*;
    use std::process::{Command, Output};

    // ------------------------------------------------------------ scratch repo

    /// 由真实 `git` 建立、隔离在 tempdir 里的仓库。
    struct Scratch {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    impl Scratch {
        fn new() -> Scratch {
            let dir = tempfile::tempdir().expect("tempdir");
            let root = dir.path().to_path_buf();
            let scratch = Scratch { _dir: dir, root };
            scratch.git_ok(&["init", "-q", "-b", "main"]);
            scratch
        }

        /// `cp -a` 出一份平行仓库（对象、index、refs、工作区全都带上），用于 A/B 对拍。
        fn copy(&self) -> Scratch {
            let dir = tempfile::tempdir().expect("tempdir");
            let root = dir.path().join("repo");
            let status = Command::new("cp")
                .arg("-a")
                .arg(&self.root)
                .arg(&root)
                .status()
                .expect("run cp");
            assert!(status.success(), "cp -a failed");
            Scratch { _dir: dir, root }
        }

        fn path(&self) -> &Path {
            &self.root
        }

        fn join(&self, rel: &str) -> PathBuf {
            self.root.join(rel)
        }

        fn repo(&self) -> Repo {
            Repo::discover(&self.root).expect("discover repo")
        }

        fn write(&self, rel: &str, contents: &str) {
            let path = self.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(path, contents).expect("write file");
        }

        fn write_bytes(&self, rel: &[u8], contents: &[u8]) {
            let path = self.root.join(OsStr::from_bytes(rel));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(path, contents).expect("write file");
        }

        fn chmod_exec(&self, rel: &str) {
            let path = self.join(rel);
            let mut perms = fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).expect("chmod");
        }

        fn symlink(&self, target: &str, rel: &str) {
            let path = self.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            std::os::unix::fs::symlink(target, path).expect("symlink");
        }

        fn git(&self, args: &[&str]) -> Output {
            Command::new("git")
                .args(args)
                .current_dir(&self.root)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", &self.root)
                .env("LC_ALL", "C")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "scenario")
                .env("GIT_AUTHOR_EMAIL", "scenario@example.com")
                .env("GIT_COMMITTER_NAME", "scenario")
                .env("GIT_COMMITTER_EMAIL", "scenario@example.com")
                .env("GIT_AUTHOR_DATE", "1700000000 +0800")
                .env("GIT_COMMITTER_DATE", "1700000000 +0800")
                .output()
                .expect("run git")
        }

        fn git_ok(&self, args: &[&str]) -> Vec<u8> {
            let out = self.git(args);
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        }

        fn git_text(&self, args: &[&str]) -> String {
            String::from_utf8(self.git_ok(args)).expect("git stdout is utf8")
        }

        fn commit_all(&self, message: &str) -> Oid {
            self.git_ok(&["add", "-A"]);
            self.git_ok(&["commit", "-q", "-m", message]);
            Oid::from_hex(self.git_text(&["rev-parse", "HEAD"]).trim()).expect("oid")
        }
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// 工作区快照：路径 → (内容字节, 可执行位)；symlink 记它的目标字节。`.git` 不看。
    fn snapshot(root: &Path) -> BTreeMap<Vec<u8>, (Vec<u8>, bool)> {
        let mut out = BTreeMap::new();
        walk(root, &mut Vec::new(), &mut out);
        out
    }

    fn walk(dir: &Path, rel: &mut Vec<u8>, out: &mut BTreeMap<Vec<u8>, (Vec<u8>, bool)>) {
        let mut names: Vec<std::ffi::OsString> = fs::read_dir(dir)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        names.sort();
        for name in names {
            if rel.is_empty() && name == ".git" {
                continue;
            }
            let path = dir.join(&name);
            let mark = rel.len();
            if mark > 0 {
                rel.push(b'/');
            }
            rel.extend_from_slice(name.as_bytes());
            let meta = fs::symlink_metadata(&path).expect("lstat");
            if meta.file_type().is_symlink() {
                let target = fs::read_link(&path).expect("readlink");
                out.insert(rel.clone(), (target.as_os_str().as_bytes().to_vec(), false));
            } else if meta.file_type().is_dir() {
                walk(&path, rel, out);
            } else {
                let content = fs::read(&path).expect("read file");
                let exec = meta.permissions().mode() & 0o111 != 0;
                out.insert(rel.clone(), (content, exec));
            }
            rel.truncate(mark);
        }
    }

    /// `git ls-tree -r -z <rev>` → 展平的 `(path, oid, mode)` 真值。
    fn git_ls_tree(scratch: &Scratch, rev: &str) -> Vec<(Vec<u8>, Oid, FileMode)> {
        let raw = scratch.git_ok(&["ls-tree", "-r", "-z", rev]);
        let mut out = Vec::new();
        for record in raw.split(|byte| *byte == 0).filter(|rec| !rec.is_empty()) {
            let tab = record.iter().position(|byte| *byte == b'\t').expect("tab");
            let header = std::str::from_utf8(&record[..tab]).expect("header utf8");
            let mut fields = header.split_whitespace();
            let mode = FileMode::from_bytes(fields.next().expect("mode").as_bytes()).expect("mode");
            let _kind = fields.next().expect("kind");
            let oid = Oid::from_hex(fields.next().expect("oid")).expect("oid");
            out.push((record[tab + 1..].to_vec(), oid, mode));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn tree_object(repo: &Repo, commit: Oid) -> Tree {
        Odb::new(repo)
            .read_object(commit_tree(repo, commit).expect("commit"))
            .expect("read tree")
            .into_tree()
            .expect("tree")
    }

    fn commit_of(repo: &Repo, rev: &str) -> Oid {
        resolve_commit(repo, rev).expect("resolve rev")
    }

    // ------------------------------------------------------------ 展平

    #[test]
    fn flatten_tree_matches_git_ls_tree() {
        if !git_available() {
            return;
        }
        let scratch = Scratch::new();
        scratch.write("a.txt", "a\n");
        scratch.write("dir/nested/deep.txt", "deep\n");
        scratch.write("dir/exec.sh", "#!/bin/sh\n");
        scratch.chmod_exec("dir/exec.sh");
        scratch.write("empty.txt", "");
        scratch.symlink("a.txt", "link.txt");
        scratch.write_bytes(b"weird\xff.txt", b"raw\n");
        scratch.commit_all("one");

        let repo = scratch.repo();
        let odb = Odb::new(&repo);
        let got = flatten_tree(
            &odb,
            commit_tree(&repo, commit_of(&repo, "HEAD")).expect("tree"),
        )
        .expect("flatten");
        assert_eq!(got, git_ls_tree(&scratch, "HEAD"));
        assert_eq!(got.len(), 6, "scenario must exercise every mode");
    }

    #[test]
    fn flatten_tree_reports_a_missing_object() {
        let scratch = Scratch::new();
        scratch.write("a.txt", "a\n");
        scratch.commit_all("one");
        let repo = scratch.repo();
        let missing = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").expect("oid");
        let err = flatten_tree(&Odb::new(&repo), missing).unwrap_err();
        assert!(matches!(err, Error::ObjectNotFound(_)), "got {err:?}");
    }

    // ------------------------------------------------------------ 端到端对拍

    /// 硬标准：`checkout_to`（= 把 tree 物化 + 重建 index + 改 HEAD）之后，
    /// 工作区、index、HEAD 与真实 `git checkout <branch>` **逐字节一致**。
    #[test]
    fn checkout_to_matches_git_checkout() {
        if !git_available() {
            return;
        }
        let a = Scratch::new();
        a.write("keep.txt", "keep\n");
        a.write("dir/one.txt", "one\n");
        a.write("dir/sub/two.txt", "two\n");
        a.write("run.sh", "#!/bin/sh\necho hi\n");
        a.chmod_exec("run.sh");
        a.write("empty.txt", "");
        a.symlink("keep.txt", "link.txt");
        a.write_bytes(b"weird\xff.txt", b"weird\n");
        a.commit_all("base");

        a.git_ok(&["branch", "feature"]);
        a.git_ok(&["checkout", "-q", "feature"]);
        a.git_ok(&["rm", "-q", "dir/sub/two.txt", "link.txt", "empty.txt"]);
        a.write("dir/one.txt", "ONE\n");
        a.write("new/deep/three.txt", "three\n");
        a.write("script", "#!/bin/sh\n");
        a.chmod_exec("script");
        a.commit_all("feature");
        a.git_ok(&["checkout", "-q", "main"]);

        let b = a.copy();
        a.git_ok(&["checkout", "-q", "feature"]);

        let repo = b.repo();
        let commit = commit_of(&repo, "refs/heads/feature");
        checkout_to(&repo, commit, false, HeadTarget::Attached("feature")).expect("mg checkout");

        assert_eq!(snapshot(a.path()), snapshot(b.path()), "worktree differs");
        assert_eq!(
            a.git_text(&["ls-files", "--stage"]),
            b.git_text(&["ls-files", "--stage"]),
            "index differs"
        );
        assert_eq!(
            a.git_text(&["status", "--porcelain"]),
            b.git_text(&["status", "--porcelain"]),
            "status differs"
        );
        assert_eq!(
            a.git_text(&["rev-parse", "HEAD"]),
            b.git_text(&["rev-parse", "HEAD"])
        );
        assert_eq!(
            fs::read(a.join(".git/HEAD")).expect("head"),
            fs::read(b.join(".git/HEAD")).expect("head")
        );
        // 反假绿：这个场景本身必须让 git 切换出「有内容变化」的结果。
        assert!(!a.join("dir/sub/two.txt").exists());
        assert!(a.join("new/deep/three.txt").is_file());
    }

    /// 本地改动「不会被覆盖」时必须原样保留（git 同），被覆盖时拒绝且一字不动。
    #[test]
    fn local_changes_survive_or_are_refused() {
        if !git_available() {
            return;
        }
        let scratch = Scratch::new();
        scratch.write("same.txt", "same\n");
        scratch.write("moved.txt", "base\n");
        scratch.commit_all("base");
        scratch.git_ok(&["branch", "feature"]);
        scratch.git_ok(&["checkout", "-q", "feature"]);
        scratch.write("moved.txt", "feature\n");
        scratch.write("added.txt", "added\n");
        scratch.commit_all("feature");
        scratch.git_ok(&["checkout", "-q", "main"]);

        // same.txt 两边相同 → 本地改动必须留下；moved.txt 本地改了且目标不同 → 拒绝。
        scratch.write("same.txt", "SAME-local\n");
        scratch.write("moved.txt", "MOVED-local\n");
        let repo = scratch.repo();
        let commit = commit_of(&repo, "refs/heads/feature");
        let tree = tree_object(&repo, commit);
        let index_before = fs::read(scratch.join(".git/index")).expect("index");

        let err = materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
            .expect_err("moved.txt must block the checkout");
        assert!(matches!(err, Error::WouldLoseChanges(_)), "got {err:?}");
        assert_eq!(
            fs::read(scratch.join("same.txt")).expect("same"),
            b"SAME-local\n"
        );
        assert_eq!(
            fs::read(scratch.join("moved.txt")).expect("moved"),
            b"MOVED-local\n"
        );
        assert_eq!(
            fs::read(scratch.join(".git/index")).expect("index"),
            index_before,
            "refusal must not touch the index"
        );

        // 清掉会被覆盖的改动 → 物化成功，且 same.txt 的本地改动仍然在。
        scratch.write("moved.txt", "base\n");
        let removed = materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
            .expect("clean checkout");
        assert!(removed.is_empty(), "nothing was tracked-but-absent");
        assert_eq!(
            fs::read(scratch.join("same.txt")).expect("same"),
            b"SAME-local\n",
            "改动不会被覆盖的文件保留不动"
        );
        assert_eq!(
            fs::read(scratch.join("moved.txt")).expect("moved"),
            b"feature\n"
        );
        assert_eq!(
            fs::read(scratch.join("added.txt")).expect("added"),
            b"added\n"
        );
    }

    /// 未跟踪文件挡住目标路径 → 拒绝（git: `untracked working tree files would be overwritten`）。
    #[test]
    fn untracked_file_in_the_way_is_refused() {
        if !git_available() {
            return;
        }
        let scratch = Scratch::new();
        scratch.write("base.txt", "base\n");
        scratch.commit_all("base");
        scratch.git_ok(&["branch", "feature"]);
        scratch.git_ok(&["checkout", "-q", "feature"]);
        scratch.write("incoming.txt", "from-feature\n");
        scratch.commit_all("feature");
        scratch.git_ok(&["checkout", "-q", "main"]);

        scratch.write("incoming.txt", "local-untracked\n");
        let repo = scratch.repo();
        let commit = commit_of(&repo, "refs/heads/feature");
        let tree = tree_object(&repo, commit);
        let before = snapshot(scratch.path());

        let err = materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
            .expect_err("untracked barrier must block");
        assert!(matches!(err, Error::WouldLoseChanges(_)), "got {err:?}");
        assert_eq!(snapshot(scratch.path()), before);

        // force = true：直接覆盖未跟踪文件（git -f 同）。
        materialize_tree(&repo, &tree, &MaterializeOptions { force: true }).expect("forced");
        assert_eq!(
            fs::read(scratch.join("incoming.txt")).expect("incoming"),
            b"from-feature\n"
        );
    }

    /// 删除行为：目标 tree 里没有的已跟踪路径从工作区消失，未跟踪 / ignored 文件留下。
    #[test]
    fn materialize_tree_reports_deletions_and_keeps_untracked_and_ignored() {
        if !git_available() {
            return;
        }
        let scratch = Scratch::new();
        scratch.write(".gitignore", "*.log\n");
        scratch.write("gone/x.txt", "gone\n");
        scratch.write("gone/y.txt", "kept\n");
        scratch.write("top.txt", "top\n");
        scratch.commit_all("base");
        scratch.git_ok(&["branch", "feature"]);
        scratch.git_ok(&["checkout", "-q", "feature"]);
        scratch.git_ok(&["rm", "-q", "gone/x.txt", "gone/y.txt"]);
        scratch.commit_all("feature");
        scratch.git_ok(&["checkout", "-q", "main"]);

        scratch.write("untracked.txt", "u\n");
        scratch.write("noise.log", "i\n");
        scratch.write("gone/extra.log", "i\n");
        let repo = scratch.repo();
        let commit = commit_of(&repo, "refs/heads/feature");
        let tree = tree_object(&repo, commit);

        let removed = materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
            .expect("materialize");
        assert_eq!(
            removed,
            vec![b"gone/x.txt".to_vec(), b"gone/y.txt".to_vec()],
            "返回值必须是本次从工作区删掉的路径（升序）"
        );
        assert!(!scratch.join("gone/x.txt").exists());
        assert!(!scratch.join("gone/y.txt").exists());
        assert!(
            scratch.join("untracked.txt").is_file(),
            "未跟踪文件必须还在"
        );
        assert!(scratch.join("noise.log").is_file(), "ignored 文件必须还在");
        assert!(
            scratch.join("gone/extra.log").is_file(),
            "被删目录里的 ignored 文件必须还在（目录因此不能被清掉）"
        );
        assert!(scratch.join("top.txt").is_file());
    }

    /// 安全：`a` 是指向工作区外的 symlink 时，物化 `a/b/c` 一个字节都不许写到外面。
    #[test]
    fn never_writes_through_a_symlink_that_leaves_the_worktree() {
        if !git_available() {
            return;
        }
        let outside = tempfile::tempdir().expect("tempdir");
        let scratch = Scratch::new();
        scratch.write("keep.txt", "keep\n");
        scratch.commit_all("base");
        scratch.git_ok(&["branch", "feature"]);
        scratch.git_ok(&["checkout", "-q", "feature"]);
        scratch.write("a/b/c.txt", "inside\n");
        scratch.commit_all("feature");
        scratch.git_ok(&["checkout", "-q", "main"]);

        // 未跟踪的 symlink 挡住 a/b/c.txt → 拒绝，且工作区外什么也没发生。
        std::os::unix::fs::symlink(outside.path(), scratch.join("a")).expect("symlink");
        let repo = scratch.repo();
        let commit = commit_of(&repo, "refs/heads/feature");
        let tree = tree_object(&repo, commit);

        let err = materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
            .expect_err("symlink barrier must block");
        assert!(matches!(err, Error::WouldLoseChanges(_)), "got {err:?}");
        assert!(fs::read_dir(outside.path())
            .expect("read_dir")
            .next()
            .is_none());

        // force = true：删掉挡路的 symlink 再建真目录，绝不跟随。
        materialize_tree(&repo, &tree, &MaterializeOptions { force: true }).expect("forced");
        assert!(
            fs::read_dir(outside.path())
                .expect("read_dir")
                .next()
                .is_none(),
            "nothing may be written outside the worktree"
        );
        assert_eq!(
            fs::read(scratch.join("a/b/c.txt")).expect("inside"),
            b"inside\n"
        );
        let canonical = fs::canonicalize(scratch.join("a")).expect("canonicalize");
        assert!(
            canonical.starts_with(fs::canonicalize(scratch.path()).expect("canonicalize")),
            "materialized directory must stay inside the worktree"
        );
    }

    /// 被**跟踪**的 symlink 变成目录：git 删掉 symlink 再建真目录（实测 E13b），
    /// 而且不允许跟随它写到工作区之外。
    #[test]
    fn tracked_symlink_is_replaced_by_a_directory_without_following_it() {
        if !git_available() {
            return;
        }
        let outside = tempfile::tempdir().expect("tempdir");
        let scratch = Scratch::new();
        std::os::unix::fs::symlink(outside.path(), scratch.join("a")).expect("symlink");
        scratch.write("keep.txt", "keep\n");
        scratch.commit_all("base");
        scratch.git_ok(&["branch", "feature"]);
        scratch.git_ok(&["checkout", "-q", "feature"]);
        scratch.git_ok(&["rm", "-q", "a"]);
        scratch.write("a/b/c.txt", "inside\n");
        scratch.commit_all("feature");
        scratch.git_ok(&["checkout", "-q", "main"]);
        assert!(fs::symlink_metadata(scratch.join("a"))
            .expect("lstat")
            .file_type()
            .is_symlink());

        let repo = scratch.repo();
        let commit = commit_of(&repo, "refs/heads/feature");
        let tree = tree_object(&repo, commit);
        let removed = materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
            .expect("tracked symlink must be removable");
        assert_eq!(
            removed,
            vec![b"a".to_vec()],
            "the symlink itself is deleted"
        );
        assert!(fs::read_dir(outside.path())
            .expect("read_dir")
            .next()
            .is_none());
        assert_eq!(
            fs::read(scratch.join("a/b/c.txt")).expect("inside"),
            b"inside\n"
        );
    }

    /// 删除路径绝不跟随 symlink 祖先（真实 git 也不跟随：非 force 拒绝，`-f` 跳过）。
    ///
    /// 真值：同一份初始状态下 `git switch prune` exit 1（`local changes would be overwritten`，
    /// 即使 symlink 后面那个文件与 index 逐字节相同），`git switch -f prune` exit 0 但
    /// **工作区外零改动、symlink 原样留下**。
    #[test]
    fn deletion_never_follows_a_symlink_ancestor() {
        if !git_available() {
            return;
        }
        let scratch = Scratch::new();
        scratch.write("a/b/c", "X\n");
        scratch.write("keep.txt", "k\n");
        scratch.commit_all("one");
        scratch.git_ok(&["branch", "prune"]);
        scratch.git_ok(&["switch", "-q", "prune"]);
        scratch.git_ok(&["rm", "-q", "a/b/c"]);
        scratch.commit_all("prune");
        scratch.git_ok(&["switch", "-q", "main"]);

        // 两份平行仓库：`mg` 跑在 A 上，真实 git 跑在 B 上；工作区里的 `a` 各自换成
        // 指向工作区外的 symlink，外面放着与 index 逐字节相同的 `a/b/c`。
        let mg_side = scratch.copy();
        let git_side = scratch.copy();
        let outside = tempfile::tempdir().expect("tempdir");
        let mut before_outside = Vec::new();
        for (index, side) in [&mg_side, &git_side].into_iter().enumerate() {
            let target = outside.path().join(format!("outside{index}"));
            fs::create_dir_all(target.join("b")).expect("mkdir");
            fs::write(target.join("b/c"), b"X\n").expect("write");
            fs::remove_dir_all(side.join("a")).expect("rm tree");
            std::os::unix::fs::symlink(&target, side.join("a")).expect("symlink");
            before_outside.push(snapshot(&target));
        }

        // 前提自检：真实 git 必须拒绝，且工作区外一字未动。
        let git_run = git_side.git(&["switch", "prune"]);
        assert!(
            !git_run.status.success(),
            "前提不成立：真实 git 竟然成功了（stderr={}）",
            String::from_utf8_lossy(&git_run.stderr)
        );
        assert_eq!(
            snapshot(&outside.path().join("outside1")),
            before_outside[1],
            "真实 git 也不许动工作区外"
        );

        let repo = mg_side.repo();
        let commit = commit_of(&repo, "refs/heads/prune");
        let tree = tree_object(&repo, commit);
        let index_before = fs::read(mg_side.join(".git/index")).expect("index");

        // 非 force：拒绝，工作区外 / symlink / index 一个字节都不动。
        let err = materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
            .expect_err("symlink ancestor must block the deletion");
        assert!(matches!(err, Error::WouldLoseChanges(_)), "got {err:?}");
        assert_eq!(
            snapshot(&outside.path().join("outside0")),
            before_outside[0],
            "mg 删掉了工作区外的内容"
        );
        assert_eq!(
            fs::read(mg_side.join(".git/index")).expect("index"),
            index_before,
            "拒绝时 index 不许动"
        );

        // force：放行（git 同），但同样绝不跟随 symlink —— 工作区外零改动、symlink 原样留下。
        let removed = materialize_tree(&repo, &tree, &MaterializeOptions { force: true })
            .expect("forced materialize");
        assert!(removed.is_empty(), "跳过的删除不能报告成删过: {removed:?}");
        assert_eq!(
            snapshot(&outside.path().join("outside0")),
            before_outside[0],
            "force 也不许动工作区外"
        );
        assert_eq!(
            fs::read_link(mg_side.join("a")).expect("readlink"),
            outside.path().join("outside0"),
            "symlink 必须原样留下"
        );

        // 真实 git 的 force 结果必须与之一致：symlink 留下、工作区外不动、工作区内容相同。
        git_side.git_ok(&["switch", "-f", "prune"]);
        assert_eq!(
            snapshot(&outside.path().join("outside1")),
            before_outside[1],
            "真实 git 的 -f 也不动工作区外"
        );
        assert_eq!(
            fs::read_link(git_side.join("a")).expect("readlink"),
            outside.path().join("outside1")
        );
        assert_eq!(
            snapshot(mg_side.path()).keys().collect::<Vec<_>>(),
            snapshot(git_side.path()).keys().collect::<Vec<_>>(),
            "两边的工作区内容必须一致"
        );
    }

    /// 写出路径遇到 symlink 祖先且顺着它 stat 不到目标：git 把 symlink 换成真目录
    /// （实测 `git switch` exit 0、`status` 干净、工作区外零改动），内容落在工作区内。
    #[test]
    fn write_replaces_a_symlink_ancestor_with_a_real_directory() {
        if !git_available() {
            return;
        }
        let scratch = Scratch::new();
        scratch.write("a/b/c", "v1\n");
        scratch.write("keep.txt", "k\n");
        scratch.commit_all("one");
        scratch.git_ok(&["branch", "feature"]);
        scratch.git_ok(&["switch", "-q", "feature"]);
        scratch.write("a/b/c", "v2\n");
        scratch.commit_all("two");
        scratch.git_ok(&["switch", "-q", "main"]);

        let mg_side = scratch.copy();
        let git_side = scratch.copy();
        let outside = tempfile::tempdir().expect("tempdir");
        let mut targets = Vec::new();
        for (index, side) in [&mg_side, &git_side].into_iter().enumerate() {
            let target = outside.path().join(format!("outside{index}"));
            fs::create_dir_all(&target).expect("mkdir");
            fs::remove_dir_all(side.join("a")).expect("rm tree");
            std::os::unix::fs::symlink(&target, side.join("a")).expect("symlink");
            targets.push(target);
        }

        // 前提自检：真实 git 成功，且把 symlink 换成了真目录。
        git_side.git_ok(&["switch", "-q", "feature"]);
        assert!(fs::symlink_metadata(git_side.join("a"))
            .expect("lstat")
            .file_type()
            .is_dir());
        assert_eq!(fs::read(git_side.join("a/b/c")).expect("git side"), b"v2\n");
        assert!(fs::read_dir(&targets[1])
            .expect("read_dir")
            .next()
            .is_none());

        let repo = mg_side.repo();
        let commit = commit_of(&repo, "refs/heads/feature");
        let tree = tree_object(&repo, commit);
        materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
            .expect("symlink ancestor must be replaced, not refused");

        assert!(fs::read_dir(&targets[0])
            .expect("read_dir")
            .next()
            .is_none());
        assert!(
            fs::canonicalize(mg_side.join("a"))
                .expect("canonicalize")
                .starts_with(fs::canonicalize(mg_side.path()).expect("canonicalize")),
            "真目录必须落在工作区内"
        );
        assert_eq!(fs::read(mg_side.join("a/b/c")).expect("mg side"), b"v2\n");
        assert_eq!(
            snapshot(mg_side.path()),
            snapshot(git_side.path()),
            "两边的工作区必须逐字节一致"
        );
    }

    /// symlink 祖先指向**工作区内**的目录时同样不跟随：非 force 拒绝，`-f` 跳过，
    /// 绝不顺着它删掉 `real/b/c`（真实 git 同为「拒绝 / 跳过」）。
    #[test]
    fn deletion_does_not_follow_a_symlink_ancestor_inside_the_worktree() {
        if !git_available() {
            return;
        }
        let scratch = Scratch::new();
        scratch.write("a/b/c", "v1\n");
        scratch.write("keep.txt", "k\n");
        scratch.commit_all("one");
        scratch.git_ok(&["branch", "prune"]);
        scratch.git_ok(&["switch", "-q", "prune"]);
        scratch.git_ok(&["rm", "-q", "a/b/c"]);
        scratch.commit_all("prune");
        scratch.git_ok(&["switch", "-q", "main"]);

        scratch.write("real/b/c", "v1\n");
        fs::remove_dir_all(scratch.join("a")).expect("rm tree");
        std::os::unix::fs::symlink("real", scratch.join("a")).expect("symlink");

        let repo = scratch.repo();
        let commit = commit_of(&repo, "refs/heads/prune");
        let tree = tree_object(&repo, commit);
        let before = snapshot(scratch.path());

        let err = materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
            .expect_err("symlink ancestor must block");
        assert!(matches!(err, Error::WouldLoseChanges(_)), "got {err:?}");
        assert_eq!(snapshot(scratch.path()), before, "拒绝时工作区一字不动");

        let removed = materialize_tree(&repo, &tree, &MaterializeOptions { force: true })
            .expect("forced materialize");
        assert!(removed.is_empty(), "跳过的删除不能报告成删过: {removed:?}");
        assert_eq!(
            fs::read(scratch.join("real/b/c")).expect("real/b/c"),
            b"v1\n",
            "绝不能顺着 symlink 删掉工作区内的 real/b/c"
        );
        assert!(fs::symlink_metadata(scratch.join("a"))
            .expect("lstat")
            .file_type()
            .is_symlink());
    }

    /// 目标提交 == 当前 HEAD 提交（且没有 `-f`）：git 只改 HEAD，index 与工作区不动。
    /// 真值来自真实 `git switch br0` 跑在同一份初始状态上（实测 P1/P3/P10）。
    #[test]
    fn switching_to_the_same_commit_only_moves_head() {
        if !git_available() {
            return;
        }
        let scratch = Scratch::new();
        scratch.write("f.txt", "A\n");
        scratch.write("k.txt", "K\n");
        scratch.commit_all("one");
        scratch.git_ok(&["branch", "br0"]);
        // MM：暂存一版、工作区又改成另一版，外加一个未跟踪文件。
        scratch.write("f.txt", "BIGGER-STAGED\n");
        scratch.git_ok(&["add", "f.txt"]);
        scratch.write("f.txt", "DIRTY-AFTER-STAGE\n");
        scratch.write("untracked.txt", "u\n");

        let other = scratch.copy();
        other.git_ok(&["switch", "br0"]);

        let repo = scratch.repo();
        let commit = commit_of(&repo, "HEAD");
        let index_before = fs::read(scratch.join(".git/index")).expect("index");
        checkout_to(&repo, commit, false, HeadTarget::Attached("br0")).expect("same-commit switch");

        assert_eq!(
            fs::read(scratch.join(".git/index")).expect("index"),
            index_before,
            "index 一个字节都不该动"
        );
        assert_eq!(
            snapshot(scratch.path()),
            snapshot(other.path()),
            "worktree differs"
        );
        assert_eq!(
            scratch.git_text(&["status", "--porcelain"]),
            other.git_text(&["status", "--porcelain"])
        );
        assert_eq!(
            scratch.git_text(&["ls-files", "--stage"]),
            other.git_text(&["ls-files", "--stage"])
        );
        assert_eq!(
            fs::read_to_string(scratch.join(".git/HEAD")).expect("head"),
            "ref: refs/heads/br0\n"
        );
        assert_eq!(
            scratch.git_text(&["status", "--porcelain"]),
            "MM f.txt\n?? untracked.txt\n"
        );

        // `-f` 则完整复位（git P4/P5）；未跟踪文件两者都不动。
        checkout_to(&repo, commit, true, HeadTarget::Attached("br0")).expect("forced");
        other.git_ok(&["switch", "-f", "br0"]);
        assert_eq!(fs::read(scratch.join("f.txt")).expect("f"), b"A\n");
        assert_eq!(
            scratch.git_text(&["status", "--porcelain"]),
            other.git_text(&["status", "--porcelain"])
        );
        assert_eq!(snapshot(scratch.path()), snapshot(other.path()));
        assert_eq!(
            scratch.git_text(&["status", "--porcelain"]),
            "?? untracked.txt\n"
        );
    }

    #[test]
    fn write_blob_to_refuses_a_symlink_parent() {
        let outside = tempfile::tempdir().expect("tempdir");
        let scratch = Scratch::new();
        scratch.write("keep.txt", "keep\n");
        scratch.commit_all("base");
        let repo = scratch.repo();
        std::os::unix::fs::symlink(outside.path(), scratch.join("link")).expect("symlink");

        let content_oid = Odb::new(&repo)
            .write(Kind::Blob, b"payload\n")
            .expect("write blob");

        let err = write_blob_to(&repo, b"link/x.txt", content_oid, FileMode::Regular)
            .expect_err("must refuse to write through a symlink");
        assert!(matches!(err, Error::WouldLoseChanges(_)), "got {err:?}");
        assert!(fs::read_dir(outside.path())
            .expect("read_dir")
            .next()
            .is_none());

        // 正常路径照样能写，且 100755 真的落了可执行位。
        write_blob_to(&repo, b"sub/exec.sh", content_oid, FileMode::Executable).expect("write");
        let meta = fs::symlink_metadata(scratch.join("sub/exec.sh")).expect("lstat");
        assert_eq!(meta.permissions().mode() & 0o777, 0o755);
    }
}
