//! `mg status` —— **T12（hermes）**（文件头的「T4（codex）」是 W1 的过期标注，以本轮任务书为准）。
//!
//! 渲染逻辑全部在 `worktree::StatusReport::porcelain`（T4 实现）。本文件只负责：
//! 1. 校验 `--porcelain` 的取值（`v2` → `NotImplemented`，不输出半成品）；
//! 2. 把 HEAD 的 tree **展平**成 `Worktree::status` 要求的「全路径条目」形式
//!    （`worktree` 层刻意不读 odb，见 `worktree/mod.rs` 的接口澄清）；
//! 3. 读 index、调用 `Worktree::status`、打印。
//!
//! 已知限制：
//! * `--porcelain=v2` 未实现（`Error::NotImplemented`）；
//! * 不带 `--porcelain`/`--short` 时 git 打印的是人类可读长格式，mg 直接打印 v1 的
//!   porcelain 行（机器可读），两者不同 —— 本轮不实现长格式；
//! * rename 检测未实现（`R ` 列），见 `worktree/status.rs`。

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::index::Index;
use crate::object::{FileMode, Object, Tree, TreeEntry};
use crate::odb::Odb;
use crate::oid::Oid;
use crate::refs::RefStore;
use crate::repo::Repo;
use crate::worktree::Worktree;

pub fn run(porcelain: Option<&str>, short: bool) -> Result<()> {
    let repo = crate::cli::open_repo()?;
    match porcelain {
        Some("v2") => return Err(Error::NotImplemented("cli::status --porcelain=v2")),
        None | Some("v1") => {}
        Some(other) => {
            return Err(Error::Other(format!(
                "unsupported --porcelain format {other:?} (v1 supports v1)"
            )))
        }
    }

    let index = Index::read(&repo)?;
    let head = head_tree(&repo)?;
    let report = Worktree::new(&repo).status(head.as_ref(), &index)?;
    // `--short` 与 `--porcelain` v1 在 git 里同形；不带参数时同上（见文件头「已知限制」）。
    let _ = short;
    print!("{}", report.porcelain());
    Ok(())
}

/// HEAD 对应的**已展平** tree（`name` = 仓库相对全路径）；没有提交（unborn HEAD）时 `None`。
pub(crate) fn head_tree(repo: &Repo) -> Result<Option<Tree>> {
    let Some(oid) = head_commit_oid(repo)? else {
        return Ok(None);
    };
    let odb = Odb::new(repo);
    let commit = odb.read_object(oid)?.into_commit()?;
    let mut entries = Vec::new();
    flatten(&odb, commit.tree, &mut Vec::new(), &mut entries)?;
    Ok(Some(Tree::new(entries)))
}

/// `path -> (oid, mode)` 形式的 HEAD 快照。
pub(crate) type HeadEntries = BTreeMap<Vec<u8>, (Oid, FileMode)>;

/// `path -> (oid, mode)` 形式（`mg rm` 判断「index 是否与 HEAD 不同」用）。
pub(crate) fn head_tree_map(repo: &Repo) -> Result<Option<HeadEntries>> {
    Ok(head_tree(repo)?.map(|tree| {
        tree.entries()
            .iter()
            .map(|entry| (entry.name.clone(), (entry.oid, entry.mode)))
            .collect()
    }))
}

/// HEAD 指向的提交 oid；unborn HEAD（ref 不存在）时 `None`。
fn head_commit_oid(repo: &Repo) -> Result<Option<Oid>> {
    match RefStore::new(repo).resolve("HEAD") {
        Ok(oid) => Ok(Some(oid)),
        Err(Error::RefNotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

/// 递归读出 tree 的所有叶条目，`name` 拼成仓库相对全路径。
fn flatten(odb: &Odb<'_>, tree: Oid, prefix: &mut Vec<u8>, out: &mut Vec<TreeEntry>) -> Result<()> {
    let object = odb.read_object(tree)?;
    let Object::Tree(subtree) = object else {
        return Err(Error::corrupt(
            "tree",
            format!("{tree} is not a tree object"),
        ));
    };
    for entry in subtree.entries() {
        let base = prefix.len();
        if base > 0 {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(&entry.name);
        if entry.mode == FileMode::Tree {
            flatten(odb, entry.oid, prefix, out)?;
        } else {
            out.push(TreeEntry {
                mode: entry.mode,
                name: prefix.clone(),
                oid: entry.oid,
            });
        }
        prefix.truncate(base);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::{Command, Output};

    use super::*;
    use crate::repo::Repo;

    fn git(dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "scenario")
            .env("GIT_AUTHOR_EMAIL", "scenario@example.com")
            .env("GIT_COMMITTER_NAME", "scenario")
            .env("GIT_COMMITTER_EMAIL", "scenario@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0800")
            .env("GIT_COMMITTER_DATE", "1700000000 +0800")
            .output()
            .expect("run git")
    }

    #[test]
    fn head_tree_is_flattened_and_matches_git() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path();
        assert!(git(dir, &["init", "-q", "-b", "main"]).status.success());
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/deep.txt"), b"deep\n").unwrap();
        std::fs::write(dir.join("top.txt"), b"top\n").unwrap();
        std::os::unix::fs::symlink("top.txt", dir.join("link")).unwrap();
        assert!(git(dir, &["add", "-A"]).status.success());
        assert!(git(dir, &["commit", "-q", "-m", "init"]).status.success());

        let repo = Repo::discover(dir).unwrap();
        let tree = head_tree(&repo).unwrap().expect("HEAD exists");
        let mut names: Vec<String> = tree
            .entries()
            .iter()
            .map(|entry| String::from_utf8(entry.name.clone()).unwrap())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a/b/deep.txt", "link", "top.txt"]);

        // 真值：`git ls-tree -r` 的 name 列。
        let raw = git(dir, &["ls-tree", "-r", "--name-only", "HEAD"]);
        let mut want: Vec<String> = String::from_utf8(raw.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        want.sort();
        assert_eq!(names, want);

        let map = head_tree_map(&repo).unwrap().expect("map");
        assert_eq!(map.len(), 3);
        assert_eq!(
            map.get(b"top.txt".as_slice()).unwrap().1,
            FileMode::Regular
        );
        assert_eq!(map.get(b"link".as_slice()).unwrap().1, FileMode::Symlink);
    }

    #[test]
    fn head_tree_is_none_in_an_unborn_repository() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = Repo::init(tmp.path(), "main").unwrap();
        assert!(head_tree(&repo).unwrap().is_none());
        assert!(head_tree_map(&repo).unwrap().is_none());
    }
}
