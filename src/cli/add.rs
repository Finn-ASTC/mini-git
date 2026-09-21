//! `mg add` —— **T12（hermes）**。
//!
//! 做的事：pathspec 展开（`.`、目录递归、单文件、多参数）、ignore 过滤、
//! 删除检测（index 里有但工作区已不存在的路径）、每个文件写 blob 后更新 index，
//! 并按 git 的 racy-git 规则填 `stat` 缓存。
//!
//! ## 硬标准（与真实 git 对拍）
//! * `mg add <paths>` 之后 `git ls-files --stage` 与真实 `git add` 完全一致；
//! * `mg add .` 与 `git add .` 一致（含 ignore 行为）；
//! * 与 `git rm` 一样，显式给出被 ignore 的路径要**拒绝**（exit != 0），
//!   但经目录递归命中的被 ignore 文件只是跳过。
//!
//! ## cache-tree（`TREE` 扩展）必须失效
//! 改过 index 条目之后，**必须**把 `TREE` 扩展（cache-tree）丢掉：它是
//! 「index 内容 → tree oid」的缓存，index 变了而它没变时，真实 git 会直接用
//! 那个过期 oid。实测（git 2.55）：手工改一条 index 条目的 oid 后保留旧
//! cache-tree，`git write-tree` 与 `git commit` 都会提交**旧 tree**，
//! 静默丢掉暂存内容。所以本文件提供 [`invalidate_tree_cache`]，
//! `mg add` / `mg rm` 共用；`mg commit` 则反过来写入正确的 tree oid。
//!
//! ## 本文件里的共享工具（同为 T12 作用域内）
//! * [`Pathspec`] / [`expand_pathspecs`]：pathspec 展开，`cli::rm` 复用；
//! * [`relativize`]：把命令行给的路径规范化成仓库相对路径，`cli::rm` 复用；
//! * [`invalidate_tree_cache`]：见上。

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry, StatData, TREE_EXTENSION};
use crate::object::Kind;
use crate::odb::Odb;
use crate::repo::Repo;
use crate::worktree::Worktree;

pub fn run(pathspec: &[PathBuf], verbose: bool) -> Result<()> {
    let repo = crate::cli::open_repo()?;
    let worktree = Worktree::new(&repo);
    let odb = Odb::new(&repo);
    let mut index = Index::read(&repo)?;
    let ignore = worktree.ignore()?;

    let specs = expand_pathspecs(&repo, pathspec, &index)?;

    // 1) 每条 pathspec 必须命中点东西（工作区里的文件/目录，或 index 里的条目）。
    for spec in &specs {
        if !spec.matches_anything(&repo, &index)? {
            return Err(Error::PathspecNotFound(spec.raw.clone()));
        }
    }

    // 2) 显式写出的、未被跟踪的 ignored 路径 → 拒绝（与 git 一致；`-f` 不在签名里）。
    let ignored: Vec<String> = specs
        .iter()
        .filter(|spec| !spec.rel.is_empty())
        .filter(|spec| !index.entries.iter().any(|entry| entry.path == spec.rel))
        .filter(|spec| {
            let is_dir = spec.is_dir_in_worktree(&repo);
            ignore.is_ignored(&spec.rel, is_dir)
        })
        .map(|spec| spec.raw.clone())
        .collect();
    if !ignored.is_empty() {
        return Err(Error::Other(format!(
            "the following paths are ignored by one of your .gitignore files: {}\nhint: mini-git `add` has no -f: drop them from .gitignore if you really want to add them",
            ignored.join(", ")
        )));
    }

    let mut dirty = false;
    let mut tree_changed = false;

    // 3) 删除检测：pathspec 覆盖范围内、index 里有但工作区已不是文件的路径。
    //    目录被整体删除时，这一轮会把该前缀下的**全部**条目删掉。
    let mut known: Vec<Vec<u8>> = index
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    known.sort();
    known.dedup();
    for path in known {
        if !specs.iter().any(|spec| spec.matches(&path)) {
            continue;
        }
        if worktree_file_exists(&repo, &path)? {
            continue;
        }
        index.remove_all_stages(&path);
        dirty = true;
        tree_changed = true;
    }

    // 4) 逐个文件写 blob → 更新 index（stat 按 git 的 racy 规则处理）。
    let scanned = worktree.scan()?;
    let now_secs = now_seconds();
    for file in &scanned {
        if file.is_dir || file.ignored {
            continue;
        }
        if !specs.iter().any(|spec| spec.matches(&file.path)) {
            continue;
        }
        let abs = repo.work_path(&file.path)?;
        let meta = std::fs::symlink_metadata(&abs)?;
        let content = worktree.read_worktree_entry(&file.path)?;
        let oid = odb.write(Kind::Blob, &content.bytes)?;

        let mut stat = StatData::from_metadata(&meta);
        // racy-git：mtime 落在 index 写入的同一秒时缓存不可信，size 必须写 0。
        // 否则真实 git 会因为「stat 命中」而跳过内容比较，漏报改动。
        if stat.mtime_s == now_secs {
            stat.size = 0;
        }

        let previous = index
            .lookup(&file.path)
            .map(|entry| (entry.oid, entry.mode));
        if index
            .entries
            .iter()
            .any(|entry| entry.path == file.path && entry.stage != 0)
        {
            // 解决冲突后 `add`：先清掉 stage 1/2/3。
            index.remove_all_stages(&file.path);
            tree_changed = true;
        }
        let mut entry = IndexEntry::new(file.path.clone(), oid, content.mode);
        entry.stat = stat;
        index.upsert(entry);
        dirty = true;
        if previous != Some((oid, content.mode)) {
            tree_changed = true;
        }
        if verbose {
            println!("add '{}'", String::from_utf8_lossy(&file.path));
        }
    }

    if !dirty {
        // 什么都没变：index 一个字节都不动（cache-tree 也不会被动坏）。
        return Ok(());
    }
    if tree_changed {
        invalidate_tree_cache(&mut index);
    }
    index.write(&repo)
}

/// 丢掉 cache-tree（`TREE` 扩展）：index 条目变了却留着它，真实 git 会拿它
/// 当「已经是这个 tree」的证据（见文件头）。原始扩展数据也要一起删，
/// 否则 `dirc::encode` 会把它原样写回去。
pub(crate) fn invalidate_tree_cache(index: &mut Index) {
    index.tree_oid = None;
    index
        .extensions
        .retain(|ext| ext.signature != TREE_EXTENSION);
}

/// 一条展开后的 pathspec（`mg add` / `mg rm` 共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pathspec {
    /// 仓库相对路径，`/` 分隔；空表示整棵树（`.`）。
    pub rel: Vec<u8>,
    /// 前缀语义（目录、`.`，或工作区里已经没了但 index 里有下级的路径）。
    pub recursive: bool,
    /// 命令行上的原始写法，只用于报错。
    pub raw: String,
}

impl Pathspec {
    pub(crate) fn matches(&self, path: &[u8]) -> bool {
        if self.rel.is_empty() {
            return true;
        }
        if path == self.rel.as_slice() {
            return true;
        }
        self.recursive && is_under(path, &self.rel)
    }

    /// pathspec 是否命中「工作区里的东西」或「index 里的条目」。
    pub(crate) fn matches_anything(&self, repo: &Repo, index: &Index) -> Result<bool> {
        if self.rel.is_empty() {
            return Ok(true);
        }
        if let Some(workdir) = repo.workdir() {
            if let Ok(rel) = String::from_utf8(self.rel.clone()) {
                if std::fs::symlink_metadata(workdir.join(rel)).is_ok() {
                    return Ok(true);
                }
            }
        }
        Ok(index.entries.iter().any(|entry| self.matches(&entry.path)))
    }

    pub(crate) fn is_dir_in_worktree(&self, repo: &Repo) -> bool {
        let Some(workdir) = repo.workdir() else {
            return false;
        };
        let Ok(rel) = String::from_utf8(self.rel.clone()) else {
            return false;
        };
        std::fs::symlink_metadata(workdir.join(rel)).is_ok_and(|meta| meta.is_dir())
    }
}

/// 把命令行上的路径展开成 [`Pathspec`]（相对路径按当前工作目录解析）。
pub(crate) fn expand_pathspecs(
    repo: &Repo,
    raw: &[PathBuf],
    index: &Index,
) -> Result<Vec<Pathspec>> {
    let mut out = Vec::with_capacity(raw.len());
    for spec in raw {
        let rel = relativize(repo, spec)?;
        let recursive = if rel.is_empty() {
            true
        } else {
            let is_dir = Pathspec {
                rel: rel.clone(),
                recursive: false,
                raw: spec.display().to_string(),
            }
            .is_dir_in_worktree(repo);
            let has_children = index
                .entries
                .iter()
                .any(|entry| is_under(&entry.path, &rel));
            is_dir || has_children
        };
        out.push(Pathspec {
            rel,
            recursive,
            raw: spec.display().to_string(),
        });
    }
    Ok(out)
}

/// 命令行路径 → 仓库相对路径（`/` 分隔字节）。`.` / 空 = 整棵树（返回空 vec）。
pub(crate) fn relativize(repo: &Repo, spec: &Path) -> Result<Vec<u8>> {
    let workdir = repo
        .workdir()
        .ok_or(Error::Unsupported("bare repository has no working tree"))?;
    let joined = if spec.is_absolute() {
        spec.to_path_buf()
    } else {
        std::env::current_dir()?.join(spec)
    };
    let normalized = normalize_lexically(&joined);
    let rel = match normalized.strip_prefix(workdir) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => {
            // 工作目录本身可能是符号链接路径，退回按「仓库相对」解释。
            let fallback = normalize_lexically(spec);
            if fallback.is_absolute() {
                return Err(Error::Other(format!(
                    "pathspec '{}' is outside the repository",
                    spec.display()
                )));
            }
            fallback
        }
    };
    path_to_rel_bytes(&rel)
}

/// 词法规范化：消掉 `.` 与 `..`（不碰文件系统）。
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(unix)]
fn path_to_rel_bytes(path: &Path) -> Result<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    Ok(path.as_os_str().as_bytes().to_vec())
}

#[cfg(not(unix))]
fn path_to_rel_bytes(path: &Path) -> Result<Vec<u8>> {
    let text = path
        .to_str()
        .ok_or_else(|| Error::Other("non-UTF8 paths are not supported".to_string()))?;
    Ok(text.as_bytes().to_vec())
}

/// `path` 是否在目录 `dir` 之下（component 边界；`dir` 为空时恒真）。
fn is_under(path: &[u8], dir: &[u8]) -> bool {
    if dir.is_empty() {
        return !path.is_empty();
    }
    path.len() > dir.len() && path.starts_with(dir) && path[dir.len()] == b'/'
}

/// 工作区里该路径存在且**不是目录**（symlink 算文件，与 git 一致）。
fn worktree_file_exists(repo: &Repo, path: &[u8]) -> Result<bool> {
    let abs = repo.work_path(path)?;
    Ok(match std::fs::symlink_metadata(&abs) {
        Ok(meta) => !meta.is_dir(),
        Err(_) => false,
    })
}

fn now_seconds() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs() as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_in(dir: &Path) -> Repo {
        Repo::init(dir, crate::repo::DEFAULT_INITIAL_BRANCH).expect("init")
    }

    #[test]
    fn is_under_uses_component_boundaries() {
        assert!(is_under(b"a/b/c", b"a"));
        assert!(is_under(b"a/b", b"a"));
        assert!(!is_under(b"ab", b"a"));
        assert!(!is_under(b"a", b"a"));
        assert!(!is_under(b"", b"a"));
        assert!(is_under(b"anything", b""));
        assert!(!is_under(b"", b""));
    }

    #[test]
    fn pathspec_matches_exact_and_recursive() {
        let exact = Pathspec {
            rel: b"a/b".to_vec(),
            recursive: false,
            raw: "a/b".to_string(),
        };
        assert!(exact.matches(b"a/b"));
        assert!(!exact.matches(b"a/b/c"));
        assert!(!exact.matches(b"a"));

        let recursive = Pathspec {
            rel: b"a/b".to_vec(),
            recursive: true,
            raw: "a/b".to_string(),
        };
        assert!(recursive.matches(b"a/b"));
        assert!(recursive.matches(b"a/b/c"));
        assert!(!recursive.matches(b"a/bc"));

        let whole = Pathspec {
            rel: Vec::new(),
            recursive: true,
            raw: ".".to_string(),
        };
        assert!(whole.matches(b"anything"));
    }

    #[test]
    fn relativize_resolves_dot_and_parent_components() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = repo_in(tmp.path());

        assert_eq!(relativize(&repo, Path::new(".")).unwrap(), Vec::<u8>::new());
        assert_eq!(
            relativize(&repo, Path::new("./")).unwrap(),
            Vec::<u8>::new()
        );
        assert_eq!(
            relativize(&repo, Path::new("a/b.txt")).unwrap(),
            b"a/b.txt".to_vec()
        );
        assert_eq!(
            relativize(&repo, Path::new("./a/../b.txt")).unwrap(),
            b"b.txt".to_vec()
        );
        // 绝对路径（工作区内的）同样接受。
        let absolute = repo.workdir().expect("workdir").join("sub/x.txt");
        assert_eq!(relativize(&repo, &absolute).unwrap(), b"sub/x.txt".to_vec());
    }

    #[test]
    fn relativize_rejects_paths_outside_the_repository() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = repo_in(tmp.path());
        let outside = tmp.path().join("../elsewhere.txt");
        assert!(relativize(&repo, &outside).is_err());
    }

    #[test]
    fn expand_pathspecs_marks_deleted_directories_recursive() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = repo_in(tmp.path());
        let mut index = Index::default();
        index.upsert(IndexEntry::new(
            b"gone/inner.txt".to_vec(),
            crate::oid::Oid::zeros(),
            crate::object::FileMode::Regular,
        ));

        let specs = expand_pathspecs(&repo, &[PathBuf::from("gone")], &index).unwrap();
        assert_eq!(specs.len(), 1);
        assert!(specs[0].recursive, "directory missing in worktree: `gone`");
        assert!(specs[0].matches(b"gone/inner.txt"));

        let specs = expand_pathspecs(&repo, &[PathBuf::from("nothing")], &index).unwrap();
        assert!(!specs[0].recursive);
        assert!(!specs[0].matches_anything(&repo, &index).unwrap());
    }

    #[test]
    fn invalidate_tree_cache_drops_the_raw_extension_too() {
        let mut index = Index {
            tree_oid: Some(crate::oid::Oid::zeros()),
            extensions: vec![
                crate::index::Extension {
                    signature: TREE_EXTENSION,
                    data: vec![0u8; 20],
                },
                crate::index::Extension {
                    signature: *b"REUC",
                    data: vec![1, 2, 3],
                },
            ],
            ..Index::default()
        };

        invalidate_tree_cache(&mut index);

        assert!(index.tree_oid.is_none());
        assert_eq!(index.extensions.len(), 1);
        assert_eq!(index.extensions[0].signature, *b"REUC");
    }
}
