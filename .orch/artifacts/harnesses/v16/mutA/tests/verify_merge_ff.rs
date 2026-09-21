//! `mg merge` 快进的差分回归（T16）。
//!
//! D1（T15 发现）：`fast_forward()` 曾把**已展平**的仓库相对路径塞进 `Tree` 的条目名，
//! 名字里带 `/`，被 `worktree::materialize::validate_name` 拒绝，于是「仓库里有子目录 +
//! 快进」直接 `fatal: corrupt tree entry name`。根因不在 T15 的写作用域，由 T16 修复。
//!
//! 本文件的可信真值只来自运行时的真实 `git`：同一初始状态复制两份，一份跑 `git merge`、
//! 一份跑 `mg merge`，逐字段比较 tree / porcelain / index stage / 工作区清单（含 mode 与
//! symlink 目标）。环境缺失一律硬失败，不提供「拿不到就 return 成通过」的分支。

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
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

fn mg_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_mg") {
        if Path::new(&path).is_file() {
            return PathBuf::from(path);
        }
    }
    panic!("mg binary not built; run `cargo test` so the bin target is available");
}

fn run_mg(dir: &Path, args: &[&str]) -> Output {
    Command::new(mg_binary())
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .expect("spawn mg")
}

fn init(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git_ok(dir, &["init", "-q", "-b", "main"]);
}

fn write_file(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn commit_all(dir: &Path, message: &str) {
    git_ok(dir, &["add", "-A"]);
    git_ok(dir, &["commit", "-qm", message]);
}

/// 递归复制，保留 symlink 与权限位（`std::fs::copy` 不保留 symlink / mode）。
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            let target = fs::read_link(&src).unwrap();
            let _ = fs::remove_file(&dst);
            std::os::unix::fs::symlink(target, &dst).unwrap();
        } else if kind.is_dir() {
            copy_tree(&src, &dst);
        } else {
            fs::copy(&src, &dst).unwrap();
            let mode = fs::metadata(&src).unwrap().permissions().mode();
            fs::set_permissions(&dst, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
}

/// 工作区清单：逐条列出 `目录` / `文件+权限` / `symlink+目标`，按路径排序，跳过 `.git`。
fn worktree_manifest(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, rel: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(base) else {
            return;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if rel.as_os_str().is_empty() && name == ".git" {
                continue;
            }
            let child_rel = rel.join(&name);
            let child = base.join(&name);
            let shown = child_rel.to_string_lossy().replace('\\', "/");
            let kind = entry.file_type().unwrap();
            if kind.is_symlink() {
                out.push(format!(
                    "L {shown} -> {}",
                    fs::read_link(&child).unwrap().display()
                ));
            } else if kind.is_dir() {
                out.push(format!("D {shown}"));
                walk(&child, &child_rel, out);
            } else {
                let mode = fs::metadata(&child).unwrap().permissions().mode() & 0o777;
                out.push(format!("F {mode:o} {shown}"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out);
    out.sort();
    out
}

fn assert_matches_git(origin: &Path, mg_side: &Path) {
    assert_eq!(
        git_text(origin, &["rev-parse", "HEAD^{tree}"]),
        git_text(mg_side, &["rev-parse", "HEAD^{tree}"]),
        "HEAD tree mismatch"
    );
    assert_eq!(
        git_text(origin, &["status", "--porcelain"]),
        git_text(mg_side, &["status", "--porcelain"]),
        "porcelain mismatch"
    );
    assert_eq!(
        git_text(origin, &["ls-files", "--stage"]),
        git_text(mg_side, &["ls-files", "--stage"]),
        "index stage mismatch"
    );
    assert_eq!(
        worktree_manifest(origin),
        worktree_manifest(mg_side),
        "worktree manifest (mode/symlink) mismatch"
    );
}

fn fork(origin: &Path, tmp: &Path) -> PathBuf {
    let mg_side = tmp.join("mg");
    copy_tree(origin, &mg_side);
    mg_side
}

fn merge_case<F>(tmp: &Path, label: &str, setup: F) -> (PathBuf, PathBuf)
where
    F: FnOnce(&Path),
{
    let origin = tmp.join(format!("origin-{label}"));
    init(&origin);
    setup(&origin);
    let mg_side = fork(&origin, &tmp.join(format!("fork-{label}")));
    (origin, mg_side)
}

#[test]
fn fast_forward_with_subdirectory_matches_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "nested-ff", |o| {
        write_file(o, "dir/b.txt", "beta\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "dir/b.txt", "beta v2\n");
        commit_all(o, "side edits b");
        git_ok(o, &["checkout", "-q", "main"]);
    });

    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        out.status.success(),
        "mg merge failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(git(origin.as_path(), &["merge", "side"]).status.success());
    assert_eq!(
        fs::read_to_string(mg_side.join("dir/b.txt")).unwrap(),
        "beta v2\n"
    );
    assert_matches_git(&origin, &mg_side);
    assert_eq!(
        git_text(&origin, &["rev-parse", "HEAD"]),
        git_text(&mg_side, &["rev-parse", "HEAD"]),
        "fast-forward must land exactly on the target commit"
    );
}

#[test]
fn fast_forward_deletes_file_in_subdirectory_matches_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "ff-delete", |o| {
        write_file(o, "dir/a.txt", "a\n");
        write_file(o, "dir/b.txt", "b\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        git_ok(o, &["rm", "-q", "dir/b.txt"]);
        commit_all(o, "side removes b");
        git_ok(o, &["checkout", "-q", "main"]);
    });

    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        out.status.success(),
        "mg merge failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(git(origin.as_path(), &["merge", "side"]).status.success());
    assert!(
        !mg_side.join("dir/b.txt").exists(),
        "side deleted dir/b.txt"
    );
    assert_matches_git(&origin, &mg_side);
}

#[test]
fn fast_forward_adds_multilevel_nested_path_matches_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "ff-add", |o| {
        write_file(o, "root.txt", "root\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "a/b/c/d.txt", "deep\n");
        write_file(o, "a/b/e.txt", "mid\n");
        commit_all(o, "side adds deep");
        git_ok(o, &["checkout", "-q", "main"]);
    });

    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        out.status.success(),
        "mg merge failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(git(origin.as_path(), &["merge", "side"]).status.success());
    assert_eq!(
        fs::read_to_string(mg_side.join("a/b/c/d.txt")).unwrap(),
        "deep\n"
    );
    assert_matches_git(&origin, &mg_side);
}

#[test]
fn fast_forward_preserves_symlink_and_executable_bit() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "ff-modes", |o| {
        write_file(o, "dir/b.txt", "beta\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        std::os::unix::fs::symlink("dir/b.txt", o.join("link")).unwrap();
        write_file(o, "run.sh", "#!/bin/sh\n");
        let run = o.join("run.sh");
        let mut perms = fs::metadata(&run).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&run, perms).unwrap();
        commit_all(o, "side adds link+exec");
        git_ok(o, &["checkout", "-q", "main"]);
    });

    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        out.status.success(),
        "mg merge failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(git(origin.as_path(), &["merge", "side"]).status.success());
    assert_eq!(
        fs::read_link(mg_side.join("link")).unwrap(),
        PathBuf::from("dir/b.txt")
    );
    assert_eq!(
        fs::metadata(mg_side.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_matches_git(&origin, &mg_side);
}

#[test]
fn conflict_in_subdirectory_still_matches_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "conflict-subdir", |o| {
        write_file(o, "dir/f.txt", "a\nb\nc\nd\ne\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "dir/f.txt", "a\nTHEIRS\nc\nd\ne\n");
        commit_all(o, "side");
        git_ok(o, &["checkout", "-q", "main"]);
        write_file(o, "dir/f.txt", "a\nOURS\nc\nd\ne\n");
        commit_all(o, "main");
    });

    let git_merge = git(origin.as_path(), &["merge", "side"]);
    assert!(!git_merge.status.success(), "git should report a conflict");
    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        !out.status.success(),
        "mg merge must exit non-zero on conflict"
    );
    assert_eq!(
        fs::read(origin.join("dir/f.txt")).unwrap(),
        fs::read(mg_side.join("dir/f.txt")).unwrap(),
        "conflict file bytes mismatch in subdirectory"
    );
    assert_matches_git(&origin, &mg_side);
    assert!(mg_side.join(".git/MERGE_HEAD").is_file());
}
