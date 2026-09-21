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

// ---------------------------------------------------------------------------
// F1（T16b）：`mg merge` 的本地改动守卫必须和真实 git 一样是**按路径**的。
// 与本次合并无关的本地改动必须被逐字节保留并放行；只有会被本次更新/删除的
// 路径上的本地改动才拒绝（拒绝时工作区/index/refs 零改动）。
// 真值仍只来自运行时真实 git；两套夹具（git/mg）初始态逐字节相同。
// ---------------------------------------------------------------------------

/// 完整状态快照：工作区清单 + HEAD + refs + index 原始字节 + MERGE_HEAD。
/// 用于断言「拒绝时一字不动」（快进拒绝场景下真实 git 也不会重写 index）。
fn snapshot(dir: &Path) -> Vec<String> {
    let mut out = worktree_manifest(dir);
    out.push(format!("HEAD {}", git_text(dir, &["rev-parse", "HEAD"])));
    out.push(format!("refs\n{}", git_text(dir, &["show-ref"])));
    out.push(format!(
        "index {:?}",
        fs::read(dir.join(".git/index")).unwrap()
    ));
    out.push(format!(
        "MERGE_HEAD {:?}",
        fs::read(dir.join(".git/MERGE_HEAD")).ok()
    ));
    out
}

/// 可观测状态快照：工作区清单 + HEAD + refs + `ls-files --stage` + MERGE_HEAD。
/// 非快进拒绝时真实 git 会刷新 index 的 stat 缓存字节（语义不变），
/// 因此这类场景按可观测状态比较，另外单独断言 mg 的 index 原始字节不变。
fn semantic_snapshot(dir: &Path) -> Vec<String> {
    let mut out = worktree_manifest(dir);
    out.push(format!("HEAD {}", git_text(dir, &["rev-parse", "HEAD"])));
    out.push(format!("refs\n{}", git_text(dir, &["show-ref"])));
    out.push(format!(
        "ls-files\n{}",
        git_text(dir, &["ls-files", "--stage"])
    ));
    out.push(format!(
        "MERGE_HEAD {:?}",
        fs::read(dir.join(".git/MERGE_HEAD")).ok()
    ));
    out
}

fn index_bytes(dir: &Path) -> Vec<u8> {
    fs::read(dir.join(".git/index")).unwrap()
}

fn assert_file_bytes_eq(origin: &Path, mg_side: &Path, rel: &str) {
    assert_eq!(
        fs::read(origin.join(rel)).unwrap(),
        fs::read(mg_side.join(rel)).unwrap(),
        "{rel}: bytes differ between git and mg"
    );
}

#[test]
fn fast_forward_preserves_unrelated_local_change_matches_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "ff-unrelated-dirty", |o| {
        write_file(o, "only.txt", "only\n");
        write_file(o, "base.txt", "base\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "s.txt", "s\n");
        write_file(o, "base.txt", "base v2\n");
        commit_all(o, "side");
        git_ok(o, &["checkout", "-q", "main"]);
    });
    for side in [&origin, &mg_side] {
        write_file(side, "only.txt", "LOCAL ONLY\n");
    }

    let git_out = git(&origin, &["merge", "side"]);
    assert!(
        git_out.status.success(),
        "premise: real git allows a fast-forward over an unrelated local change: {}",
        String::from_utf8_lossy(&git_out.stderr)
    );
    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        out.status.success(),
        "mg must not refuse an unrelated local change: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Fast-forward"),
        "mg must report a fast-forward"
    );

    assert_matches_git(&origin, &mg_side);
    assert_file_bytes_eq(&origin, &mg_side, "only.txt");
    assert_eq!(
        fs::read(mg_side.join("only.txt")).unwrap(),
        b"LOCAL ONLY\n",
        "the unrelated local change must survive byte for byte"
    );
    assert_eq!(fs::read_to_string(mg_side.join("s.txt")).unwrap(), "s\n");
    assert_eq!(
        git_text(&origin, &["rev-parse", "HEAD"]),
        git_text(&mg_side, &["rev-parse", "HEAD"]),
        "the fast-forward must land on the same commit"
    );
}

#[test]
fn fast_forward_refuses_local_change_on_updated_path_matches_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "ff-touched-dirty", |o| {
        write_file(o, "base.txt", "base\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "base.txt", "base v2\n");
        write_file(o, "s.txt", "s\n");
        commit_all(o, "side");
        git_ok(o, &["checkout", "-q", "main"]);
    });
    for side in [&origin, &mg_side] {
        write_file(side, "base.txt", "LOCAL DIRTY\n");
    }
    let (before_origin, before_mg) = (snapshot(&origin), snapshot(&mg_side));

    let git_out = git(&origin, &["merge", "side"]);
    assert!(
        !git_out.status.success(),
        "premise: real git refuses to overwrite a local change on an updated path"
    );
    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        !out.status.success(),
        "mg must refuse a local change on a path it would update"
    );

    assert_eq!(
        snapshot(&origin),
        before_origin,
        "git side must be untouched"
    );
    assert_eq!(snapshot(&mg_side), before_mg, "mg side must be untouched");
    assert_file_bytes_eq(&origin, &mg_side, "base.txt");
    assert!(
        !mg_side.join("s.txt").exists(),
        "nothing from the incoming commit may be materialized on refusal"
    );
}

#[test]
fn non_fast_forward_preserves_unrelated_local_change_matches_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "nonff-unrelated-dirty", |o| {
        write_file(o, "root.txt", "root\n");
        write_file(o, "only.txt", "only\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "side.txt", "side\n");
        commit_all(o, "side");
        git_ok(o, &["checkout", "-q", "main"]);
        write_file(o, "main.txt", "main\n");
        commit_all(o, "main");
    });
    for side in [&origin, &mg_side] {
        write_file(side, "only.txt", "LOCAL ONLY\n");
    }

    let git_out = git(&origin, &["merge", "side"]);
    assert!(
        git_out.status.success(),
        "premise: real git performs a non-fast-forward merge over an unrelated local change: {}",
        String::from_utf8_lossy(&git_out.stderr)
    );
    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        out.status.success(),
        "mg must not refuse an unrelated local change on a real merge: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_matches_git(&origin, &mg_side);
    assert_file_bytes_eq(&origin, &mg_side, "only.txt");
    assert_eq!(
        fs::read(mg_side.join("only.txt")).unwrap(),
        b"LOCAL ONLY\n",
        "the unrelated local change must survive byte for byte"
    );
    assert_eq!(
        git_text(&mg_side, &["log", "--format=%P", "-1"])
            .split_whitespace()
            .count(),
        2,
        "a merge commit with two parents"
    );
}

#[test]
fn non_fast_forward_refuses_local_change_on_updated_path_matches_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "nonff-touched-dirty", |o| {
        write_file(o, "root.txt", "root\n");
        write_file(o, "anchor.txt", "anchor\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "root.txt", "root side\n");
        commit_all(o, "side");
        git_ok(o, &["checkout", "-q", "main"]);
        write_file(o, "main.txt", "main\n");
        commit_all(o, "main");
    });
    for side in [&origin, &mg_side] {
        write_file(side, "root.txt", "LOCAL DIRTY\n");
    }
    let before_origin = semantic_snapshot(&origin);
    let before_mg = semantic_snapshot(&mg_side);
    let mg_index = index_bytes(&mg_side);

    let git_out = git(&origin, &["merge", "side"]);
    assert!(
        !git_out.status.success(),
        "premise: real git refuses to overwrite a local change on a merged path"
    );
    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        !out.status.success(),
        "mg must refuse a local change on a path it would write"
    );

    assert_eq!(
        semantic_snapshot(&origin),
        before_origin,
        "git side must be untouched"
    );
    assert_eq!(
        semantic_snapshot(&mg_side),
        before_mg,
        "mg side must be untouched"
    );
    assert_eq!(
        index_bytes(&mg_side),
        mg_index,
        "mg must not rewrite the index"
    );
    assert_file_bytes_eq(&origin, &mg_side, "root.txt");
    assert!(
        !mg_side.join(".git/MERGE_HEAD").exists(),
        "a refused merge must not leave merge state behind"
    );
}

#[test]
fn untracked_file_blocking_incoming_path_is_refused_matches_git() {
    // 快进：目标新增的文件在本地未跟踪、内容不同 → 双方拒绝。
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "ff-untracked-block", |o| {
        write_file(o, "base.txt", "base\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "s.txt", "incoming\n");
        commit_all(o, "side");
        git_ok(o, &["checkout", "-q", "main"]);
    });
    for side in [&origin, &mg_side] {
        write_file(side, "s.txt", "LOCAL UNTRACKED\n");
    }
    let before_origin = snapshot(&origin);
    let before_mg = snapshot(&mg_side);

    assert!(
        !git(&origin, &["merge", "side"]).status.success(),
        "premise: real git refuses to overwrite an untracked file"
    );
    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        !out.status.success(),
        "mg must refuse an untracked overwrite"
    );
    assert_eq!(snapshot(&origin), before_origin);
    assert_eq!(snapshot(&mg_side), before_mg);
    assert_eq!(
        fs::read(mg_side.join("s.txt")).unwrap(),
        b"LOCAL UNTRACKED\n"
    );

    // 非快进：同样的未跟踪阻塞。
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "nonff-untracked-block", |o| {
        write_file(o, "base.txt", "base\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "s.txt", "incoming\n");
        commit_all(o, "side");
        git_ok(o, &["checkout", "-q", "main"]);
        write_file(o, "main.txt", "main\n");
        commit_all(o, "main");
    });
    for side in [&origin, &mg_side] {
        write_file(side, "s.txt", "LOCAL UNTRACKED\n");
    }
    let before_origin = snapshot(&origin);
    let before_mg = snapshot(&mg_side);

    assert!(
        !git(&origin, &["merge", "side"]).status.success(),
        "premise: real git refuses an untracked overwrite on a real merge"
    );
    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        !out.status.success(),
        "mg must refuse an untracked overwrite"
    );
    assert_eq!(snapshot(&origin), before_origin);
    assert_eq!(snapshot(&mg_side), before_mg);
    assert_eq!(
        fs::read(mg_side.join("s.txt")).unwrap(),
        b"LOCAL UNTRACKED\n"
    );
}

#[test]
fn conflicts_with_unrelated_local_change_match_git() {
    for label in ["uu", "aa", "du", "ud"] {
        let tmp = tempfile::tempdir().unwrap();
        let (origin, mg_side) = merge_case(tmp.path(), &format!("conflict-dirty-{label}"), |o| {
            write_file(o, "conflict.txt", "a\nb\nc\nd\ne\n");
            write_file(o, "anchor.txt", "anchor\n");
            write_file(o, "only.txt", "only\n");
            commit_all(o, "base");
            git_ok(o, &["checkout", "-q", "-b", "side"]);
            match label {
                "uu" => write_file(o, "conflict.txt", "a\nTHEIRS\nc\nd\ne\n"),
                "aa" => write_file(o, "both.txt", "SIDE\n"),
                "du" => write_file(o, "conflict.txt", "theirs modifies\n"),
                _ => {
                    git_ok(o, &["rm", "-q", "conflict.txt"]);
                }
            }
            commit_all(o, "side");
            git_ok(o, &["checkout", "-q", "main"]);
            match label {
                "uu" => write_file(o, "conflict.txt", "a\nOURS\nc\nd\ne\n"),
                "aa" => write_file(o, "both.txt", "MAIN\n"),
                "du" => {
                    git_ok(o, &["rm", "-q", "conflict.txt"]);
                }
                _ => write_file(o, "conflict.txt", "ours modifies\n"),
            }
            commit_all(o, "main");
        });
        // 与本次冲突无关的本地改动：必须在双方都被逐字节保留。
        for side in [&origin, &mg_side] {
            write_file(side, "only.txt", "LOCAL ONLY\n");
        }

        let git_out = git(&origin, &["merge", "side"]);
        assert!(
            !git_out.status.success(),
            "premise: git conflicts ({label})"
        );
        let out = run_mg(&mg_side, &["merge", "side"]);
        assert!(
            !out.status.success(),
            "mg must surface the conflict ({label})"
        );

        assert_matches_git(&origin, &mg_side);
        assert_file_bytes_eq(&origin, &mg_side, "only.txt");
        assert_eq!(
            fs::read(mg_side.join("only.txt")).unwrap(),
            b"LOCAL ONLY\n",
            "case {label}: the unrelated local change must survive"
        );
        assert_eq!(
            git_text(&origin, &["status", "--porcelain"]),
            git_text(&mg_side, &["status", "--porcelain"]),
            "case {label}: porcelain (UU/AA/DU/UD) must match"
        );
        if origin.join("conflict.txt").exists() {
            assert_eq!(
                fs::read(origin.join("conflict.txt")).unwrap(),
                fs::read(mg_side.join("conflict.txt")).unwrap(),
                "case {label}: conflict bytes must match"
            );
        }
        if origin.join("both.txt").exists() {
            assert_eq!(
                fs::read(origin.join("both.txt")).unwrap(),
                fs::read(mg_side.join("both.txt")).unwrap(),
                "case {label}: add/add bytes must match"
            );
        }
    }
}

#[test]
fn fast_forward_preserves_unrelated_staged_change_matches_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "ff-staged-unrelated", |o| {
        write_file(o, "only.txt", "only\n");
        write_file(o, "base.txt", "base\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "base.txt", "base v2\n");
        commit_all(o, "side");
        git_ok(o, &["checkout", "-q", "main"]);
    });
    for side in [&origin, &mg_side] {
        write_file(side, "only.txt", "STAGED ONLY\n");
        git_ok(side, &["add", "only.txt"]);
    }

    let git_out = git(&origin, &["merge", "side"]);
    assert!(
        git_out.status.success(),
        "premise: real git fast-forwards over an unrelated staged change"
    );
    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        out.status.success(),
        "mg must not refuse an unrelated staged change: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_matches_git(&origin, &mg_side);
    assert_eq!(
        git_text(&mg_side, &["status", "--porcelain"]),
        "M  only.txt",
        "the staged change must stay staged"
    );
    assert_eq!(
        fs::read(mg_side.join("only.txt")).unwrap(),
        b"STAGED ONLY\n"
    );
}

#[test]
fn non_fast_forward_refuses_staged_change_like_git() {
    let tmp = tempfile::tempdir().unwrap();
    let (origin, mg_side) = merge_case(tmp.path(), "nonff-staged", |o| {
        write_file(o, "root.txt", "root\n");
        write_file(o, "only.txt", "only\n");
        commit_all(o, "base");
        git_ok(o, &["checkout", "-q", "-b", "side"]);
        write_file(o, "side.txt", "side\n");
        commit_all(o, "side");
        git_ok(o, &["checkout", "-q", "main"]);
        write_file(o, "main.txt", "main\n");
        commit_all(o, "main");
    });
    for side in [&origin, &mg_side] {
        write_file(side, "only.txt", "STAGED ONLY\n");
        git_ok(side, &["add", "only.txt"]);
    }
    let before_origin = semantic_snapshot(&origin);
    let before_mg = semantic_snapshot(&mg_side);
    let mg_index = index_bytes(&mg_side);

    let git_out = git(&origin, &["merge", "side"]);
    assert!(
        !git_out.status.success(),
        "premise: real git refuses a real merge while the index has staged changes"
    );
    let out = run_mg(&mg_side, &["merge", "side"]);
    assert!(
        !out.status.success(),
        "mg must refuse a real merge while the index has staged changes"
    );

    assert_eq!(
        semantic_snapshot(&origin),
        before_origin,
        "git side must be untouched"
    );
    assert_eq!(
        semantic_snapshot(&mg_side),
        before_mg,
        "mg side must be untouched"
    );
    assert_eq!(
        index_bytes(&mg_side),
        mg_index,
        "mg must not rewrite the index"
    );
    assert_eq!(
        fs::read(mg_side.join("only.txt")).unwrap(),
        b"STAGED ONLY\n"
    );
    assert!(!mg_side.join("side.txt").exists());
}
