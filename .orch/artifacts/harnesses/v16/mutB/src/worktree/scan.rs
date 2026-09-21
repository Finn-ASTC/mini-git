//! 工作区文件扫描。**T4（codex）实现范围**。
//!
//! 要求：
//! * 不进入 `.git` 目录。
//! * 整个目录被 ignore 时不要下降（git 依赖这个行为折叠 untracked 目录）。
//! * 路径用 `/` 分隔的仓库相对 UTF-8 字节。
//! * symlink 不跟随（记为文件，mode 由 `read_worktree_entry` 决定）。
//! * 空目录不产出条目（git 不跟踪目录）。
//!
//! 实现说明：
//! * 用 `symlink_metadata` 判断类型，因此指向目录的 symlink 不会被下降
//!   （git 同样不跟随工作区里的 symlink）。
//! * 被 ignore 的目录只产出自身一条（`is_dir = true, ignored = true`）并停止下降；
//!   这是 status 折叠 `?? dir/` 的前提。
//! * 返回前按 path 字节序排序（status 直接复用这个顺序，不再重排）。
//! * 非 UTF-8 文件名按原始字节保留（`Repo::work_path` 目前只支持 UTF-8，属已知限制）。

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::repo::Repo;

use super::ignore::{join_rel, os_bytes, Ignore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeFile {
    pub path: Vec<u8>,
    pub ignored: bool,
    pub is_dir: bool,
}

pub fn scan_worktree(repo: &Repo) -> Result<Vec<WorktreeFile>> {
    let Some(workdir) = repo.workdir() else {
        // bare 仓库没有工作区。
        return Ok(Vec::new());
    };
    let ignore = Ignore::load(repo)?;
    let mut out = Vec::new();
    scan_dir(workdir, &[], &ignore, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn scan_dir(
    dir: &Path,
    rel: &[u8],
    ignore: &Ignore,
    out: &mut Vec<WorktreeFile>,
) -> Result<()> {
    let mut entries: Vec<(Vec<u8>, PathBuf, fs::Metadata)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = os_bytes(&entry.file_name());
        if name == b".git" {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path())?;
        entries.push((name, entry.path(), meta));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, path, meta) in entries {
        let child = join_rel(rel, &name);
        if meta.is_dir() {
            if ignore.is_ignored(&child, true) {
                out.push(WorktreeFile {
                    path: child,
                    ignored: true,
                    is_dir: true,
                });
                continue;
            }
            scan_dir(&path, &child, ignore, out)?;
        } else {
            let ignored = ignore.is_ignored(&child, false);
            out.push(WorktreeFile {
                path: child,
                ignored,
                is_dir: false,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanned_paths(repo: &Repo) -> Vec<(String, bool, bool)> {
        scan_worktree(repo)
            .expect("scan")
            .into_iter()
            .map(|f| {
                (
                    String::from_utf8_lossy(&f.path).into_owned(),
                    f.is_dir,
                    f.ignored,
                )
            })
            .collect()
    }

    #[test]
    fn empty_directory_yields_no_entry_and_git_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repo::init(dir.path(), "main").expect("init");
        std::fs::create_dir_all(dir.path().join("empty")).unwrap();
        assert!(scanned_paths(&repo).is_empty(), ".git and empty dirs are invisible");
    }

    #[test]
    fn nested_files_are_relative_slash_separated_and_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repo::init(dir.path(), "main").expect("init");
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("z.txt"), b"z").unwrap();
        std::fs::write(dir.path().join("a/b/c.txt"), b"c").unwrap();
        std::fs::write(dir.path().join("a/x.txt"), b"x").unwrap();

        let paths: Vec<String> = scanned_paths(&repo).into_iter().map(|p| p.0).collect();
        assert_eq!(paths, vec!["a/b/c.txt", "a/x.txt", "z.txt"]);
    }

    #[test]
    fn ignored_file_is_flagged_and_ignored_dir_is_not_descended() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repo::init(dir.path(), "main").expect("init");
        std::fs::write(dir.path().join(".gitignore"), b"*.log\nbuild/\n").unwrap();
        std::fs::write(dir.path().join("debug.log"), b"l").unwrap();
        std::fs::create_dir_all(dir.path().join("build/deep")).unwrap();
        std::fs::write(dir.path().join("build/deep/x.txt"), b"x").unwrap();
        std::fs::write(dir.path().join("ok.txt"), b"o").unwrap();

        let entries = scanned_paths(&repo);
        assert_eq!(
            entries,
            vec![
                (".gitignore".to_string(), false, false),
                ("build".to_string(), true, true),
                ("debug.log".to_string(), false, true),
                ("ok.txt".to_string(), false, false),
            ]
        );
    }

    #[test]
    fn symlinks_are_not_followed_or_descended() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repo::init(dir.path(), "main").expect("init");
        std::fs::create_dir_all(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/inner.txt"), b"i").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("real", dir.path().join("link")).unwrap();
            let entries = scanned_paths(&repo);
            let link = entries.iter().find(|e| e.0 == "link").expect("link entry");
            assert!(!link.1, "symlink must be reported as a non-directory");
            let mut names: Vec<&str> = entries.iter().map(|e| e.0.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, vec!["link", "real/inner.txt"]);
        }
    }
}
