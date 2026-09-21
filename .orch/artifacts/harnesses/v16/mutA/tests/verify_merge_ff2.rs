//! **V16 独立验证**（T16 / 缺陷 D1）：`mg merge` 快进在含子目录的仓库里必须与真实 git 一致。
//!
//! 本文件与作者 `tests/verify_merge_ff.rs` 是**两套平行实现**（不复用其任何 helper）。
//! 真值只有两个来源：运行时真实 `git` 进程，以及文件系统自身。没有任何字段来自
//! mg 的自述；环境缺失 / 二进制缺失一律硬失败，绝不 `return` 成通过。
//!
//! 目录约定：每个用例先用**真实 git** 造出初始拓扑，再用 `cp -a` 复制成两份
//! （`.git`、index 一并复制），一份跑 `git`、一份跑 `mg`，然后逐字段比较：
//! `HEAD` oid / `HEAD tree` / merge parents / `status --porcelain` / `ls-files --stage`
//! / `show-ref` / 工作区清单（含权限位与 symlink 目标）/ `git fsck` 诊断。

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// ---------------------------------------------------------------------------
// 进程封装
// ---------------------------------------------------------------------------

/// 真实 git。**不** export 任何 `GIT_*`；隔离与身份只作用于子进程，并被固定以保证可复现。
fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "V16")
        .env("GIT_AUTHOR_EMAIL", "v16@example.com")
        .env("GIT_COMMITTER_NAME", "V16")
        .env("GIT_COMMITTER_EMAIL", "v16@example.com")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
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
        .trim_end()
        .to_string()
}

/// 被测二进制：cargo 为 integration test 提供的路径。缺失即硬失败（不许跳过）。
fn mg_binary() -> PathBuf {
    match std::env::var("CARGO_BIN_EXE_mg") {
        Ok(path) if Path::new(&path).is_file() => PathBuf::from(path),
        _ => panic!("CARGO_BIN_EXE_mg is not set/is not a file; `cargo test` must build the bin target"),
    }
}

fn mg(dir: &Path, args: &[&str]) -> Output {
    Command::new(mg_binary())
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_PAGER", "cat")
        .args(args)
        .output()
        .expect("spawn mg")
}

fn mg_ok(dir: &Path, args: &[&str]) -> Output {
    let out = mg(dir, args);
    assert!(
        out.status.success(),
        "mg {args:?} failed in {}:\nstdout={}\nstderr={}",
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn write_file(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(&path, contents).expect("write file");
}

fn write_exec(dir: &Path, rel: &str, contents: &str) {
    write_file(dir, rel, contents);
    let path = dir.join(rel);
    let mut perms = fs::metadata(&path).expect("stat").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod 755");
}

fn commit_all(dir: &Path, message: &str) {
    git_ok(dir, &["add", "-A"]);
    git_ok(dir, &["commit", "-q", "-m", message]);
}

/// 逐字节复制（保留权限位与 symlink；`std::fs::copy` 不保留 symlink）。
fn copy_tree(from: &Path, to: &Path) {
    assert!(!to.exists(), "copy destination already exists: {}", to.display());
    let status = Command::new("cp")
        .arg("-a")
        .arg(from)
        .arg(to)
        .status()
        .expect("spawn cp");
    assert!(status.success(), "cp -a {} -> {} failed", from.display(), to.display());
}

/// 用真实 git 造初始拓扑，然后复制成 `git` / `mg` 两份**逐字节相同**的仓库。
fn two_copies<F>(tmp: &Path, label: &str, setup: F) -> (PathBuf, PathBuf)
where
    F: FnOnce(&Path),
{
    let git_side = tmp.join(format!("{label}-git"));
    fs::create_dir_all(&git_side).expect("mkdir");
    git_ok(&git_side, &["init", "-q", "-b", "main"]);
    setup(&git_side);
    let mg_side = tmp.join(format!("{label}-mg"));
    copy_tree(&git_side, &mg_side);
    (git_side, mg_side)
}

// ---------------------------------------------------------------------------
// 观测原语（全部经过真实 git / 文件系统）
// ---------------------------------------------------------------------------

fn head_oid(dir: &Path) -> String {
    git_text(dir, &["rev-parse", "HEAD"])
}

fn head_tree(dir: &Path) -> String {
    git_text(dir, &["rev-parse", "HEAD^{tree}"])
}

fn parents(dir: &Path) -> String {
    git_text(dir, &["log", "--format=%P", "-1"])
}

fn porcelain(dir: &Path) -> String {
    git_text(dir, &["status", "--porcelain"])
}

fn index_stage(dir: &Path) -> String {
    git_text(dir, &["ls-files", "--stage"])
}

fn refs(dir: &Path) -> String {
    git_text(dir, &["show-ref"])
}

/// 递归清单：`D <目录>` / `F <权限> <文件>` / `L <symlink> -> <目标>`，跳过顶层 `.git`。
fn manifest(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, rel: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(base.join(rel)).expect("read_dir") {
            let entry = entry.expect("dirent");
            let name = entry.file_name();
            if rel.as_os_str().is_empty() && name == ".git" {
                continue;
            }
            let child_rel = rel.join(&name);
            let shown = child_rel.to_string_lossy().replace('\\', "/");
            let md = fs::symlink_metadata(entry.path()).expect("symlink_metadata");
            let kind = md.file_type();
            if kind.is_symlink() {
                let target = fs::read_link(entry.path()).expect("read_link");
                out.push(format!("L {shown} -> {}", target.display()));
            } else if kind.is_dir() {
                out.push(format!("D {shown}"));
                walk(base, &child_rel, out);
            } else {
                out.push(format!("F {:o} {shown}", md.permissions().mode() & 0o777));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out);
    out.sort();
    out
}

/// `git fsck` 的诊断行（dangling/notice 不算问题）。
fn fsck_problems(dir: &Path) -> Vec<String> {
    let out = git(dir, &["fsck", "--no-progress", "--strict"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let mut problems = Vec::new();
    for line in stdout.lines().chain(stderr.lines()) {
        let line = line.trim();
        if line.is_empty() || line.starts_with("dangling") || line.starts_with("notice:") {
            continue;
        }
        problems.push(line.to_string());
    }
    problems
}

/// 完整状态快照：工作区清单 + HEAD + 全部 refs + index 原始字节 + MERGE_HEAD 状态。
/// 用于断言「拒绝时一字不动」。
fn snapshot(dir: &Path) -> Vec<String> {
    let mut out = manifest(dir);
    out.push(format!(
        "HEAD file: {:?}",
        fs::read(dir.join(".git/HEAD")).expect("read .git/HEAD")
    ));
    out.push(format!("head oid: {}", head_oid(dir)));
    out.push(format!("refs:\n{}", refs(dir)));
    out.push(format!(
        "index bytes: {:?}",
        fs::read(dir.join(".git/index")).expect("read .git/index")
    ));
    out.push(format!(
        "MERGE_HEAD: {:?}",
        fs::read(dir.join(".git/MERGE_HEAD")).ok()
    ));
    out
}

/// 真实 `git pull` 给冲突标记的「theirs」标签是 fetch 到的 **oid**（FETCH_HEAD 没有 ref 名），
/// 而 `git merge <ref>` / `mg pull` 用 **ref 名**。正文比较时把标签归一化，
/// 标签本身的差异单独作为观测上报（不属于 D1，属 pull 的标签选择）。
fn normalize_conflict_labels(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).to_string();
    text.lines()
        .map(|line| {
            if line.starts_with("<<<<<<< ") {
                "<<<<<<< <label>".to_string()
            } else if line.starts_with(">>>>>>> ") {
                ">>>>>>> <label>".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn theirs_label(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .find_map(|line| line.strip_prefix(">>>>>>> ").map(str::to_string))
}

/// `git merge-base --is-ancestor` 的退出码：0 = 是祖先，1 = 不是，其余 = 参数/对象缺失出错。
fn ancestor_exit_code(dir: &Path, ancestor: &str, descendant: &str) -> Option<i32> {
    git(dir, &["merge-base", "--is-ancestor", ancestor, descendant])
        .status
        .code()
}

// ---------------------------------------------------------------------------
// 对拍断言
// ---------------------------------------------------------------------------

/// 两边都必须能被真实 git 读干净。
fn assert_both_fsck_clean(git_side: &Path, mg_side: &Path, label: &str) {
    assert_eq!(
        fsck_problems(mg_side),
        Vec::<String>::new(),
        "{label}: git fsck found problems in the mg side"
    );
    assert_eq!(
        fsck_problems(git_side),
        Vec::<String>::new(),
        "{label}: git fsck found problems in the git side"
    );
}

fn assert_side_state_equal(git_side: &Path, mg_side: &Path, label: &str) {
    assert_eq!(head_tree(mg_side), head_tree(git_side), "{label}: HEAD tree");
    assert_eq!(porcelain(mg_side), porcelain(git_side), "{label}: porcelain");
    assert_eq!(
        index_stage(mg_side),
        index_stage(git_side),
        "{label}: ls-files --stage"
    );
    assert_eq!(
        manifest(mg_side),
        manifest(git_side),
        "{label}: worktree manifest (dirs / mode / symlink targets)"
    );
    assert_both_fsck_clean(git_side, mg_side, label);
}

/// 快进：还必须落在**同一个提交**上，且 refs 完全一致。
fn assert_fast_forward_equal(git_side: &Path, mg_side: &Path, label: &str) {
    assert_eq!(head_oid(mg_side), head_oid(git_side), "{label}: HEAD oid");
    assert_eq!(parents(mg_side), parents(git_side), "{label}: HEAD parents");
    assert_eq!(refs(mg_side), refs(git_side), "{label}: refs");
    assert_side_state_equal(git_side, mg_side, label);
}

// ---------------------------------------------------------------------------
// 0. 不许假绿：确认测试真的在调 mg 二进制、真值真的来自真实 git
// ---------------------------------------------------------------------------

#[test]
fn selfcheck_harness_runs_the_real_mg_binary_and_real_git() {
    let bin = mg_binary();
    assert!(bin.is_file(), "mg binary missing: {}", bin.display());

    let version = Command::new(&bin)
        .arg("--version")
        .output()
        .expect("spawn mg --version");
    assert!(version.status.success(), "`mg --version` must succeed");
    let text = String::from_utf8_lossy(&version.stdout).to_string();
    assert!(
        text.to_lowercase().contains("mg"),
        "`mg --version` must identify itself, got {text:?}"
    );

    // 一个真正解析参数表的 CLI 必须拒绝未知子命令；恒成功/空壳会在这里暴露。
    let bogus = Command::new(&bin)
        .arg("v16-not-a-real-subcommand")
        .output()
        .expect("spawn mg <bogus>");
    assert!(
        !bogus.status.success(),
        "mg accepted an unknown subcommand — not the real CLI"
    );

    // 真值侧自检：真实 git 可用，且能建出仓库。
    let tmp = tempfile::tempdir().expect("tempdir");
    let probe = tmp.path().join("probe");
    fs::create_dir_all(&probe).expect("mkdir");
    git_ok(&probe, &["init", "-q", "-b", "main"]);
    assert_eq!(
        git_text(&probe, &["rev-parse", "--is-inside-work-tree"]),
        "true"
    );
    assert_eq!(
        git_text(&probe, &["--version"]).split_whitespace().next(),
        Some("git")
    );
}

// ---------------------------------------------------------------------------
// 1. 核心缺陷 D1：含子目录 + 快进
// ---------------------------------------------------------------------------

#[test]
fn fast_forward_with_subdirectory_matches_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (git_side, mg_side) = two_copies(tmp.path(), "d1-core", |repo| {
        write_file(repo, "dir/b.txt", "beta\n");
        write_file(repo, "root.txt", "root\n");
        commit_all(repo, "base");
        git_ok(repo, &["checkout", "-q", "-b", "side"]);
        write_file(repo, "dir/b.txt", "beta v2\n");
        write_file(repo, "s.txt", "s\n");
        commit_all(repo, "side edits dir/b.txt and adds s.txt");
        git_ok(repo, &["checkout", "-q", "main"]);
    });

    // 前提自检：真值侧确实是快进（否则本用例没在测快进）。
    let base = head_oid(&git_side);
    let target = git_text(&git_side, &["rev-parse", "refs/heads/side"]);
    assert!(
        git(&git_side, &["merge-base", "--is-ancestor", &base, &target])
            .status
            .success(),
        "premise broken: main is not an ancestor of side"
    );

    let git_out = git(&git_side, &["merge", "side"]);
    let mg_out = mg(&mg_side, &["merge", "side"]);
    assert!(git_out.status.success(), "premise: real git fast-forwards");
    assert!(
        mg_out.status.success(),
        "mg merge failed: {}",
        String::from_utf8_lossy(&mg_out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&mg_out.stdout).contains("Fast-forward"),
        "mg must report a fast-forward, got {:?}",
        String::from_utf8_lossy(&mg_out.stdout)
    );

    assert_fast_forward_equal(&git_side, &mg_side, "core nested fast-forward");
    assert_eq!(
        fs::read_to_string(mg_side.join("dir/b.txt")).expect("dir/b.txt"),
        "beta v2\n"
    );
    assert_eq!(fs::read_to_string(mg_side.join("s.txt")).expect("s.txt"), "s\n");
}

// ---------------------------------------------------------------------------
// 2. 快进删除子目录里的最后一个文件 → 空目录必须被清理
// ---------------------------------------------------------------------------

#[test]
fn fast_forward_deleting_last_file_prunes_empty_dir_matches_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (git_side, mg_side) = two_copies(tmp.path(), "d1-delete", |repo| {
        write_file(repo, "dir/keep.txt", "keep\n");
        write_file(repo, "dir/b.txt", "b\n");
        write_file(repo, "gone/only.txt", "only\n");
        commit_all(repo, "base");
        git_ok(repo, &["checkout", "-q", "-b", "side"]);
        git_ok(repo, &["rm", "-q", "gone/only.txt"]);
        write_file(repo, "dir/keep.txt", "keep v2\n");
        commit_all(repo, "side deletes gone/only.txt");
        git_ok(repo, &["checkout", "-q", "main"]);
    });

    let git_out = git(&git_side, &["merge", "side"]);
    let mg_out = mg(&mg_side, &["merge", "side"]);
    assert!(git_out.status.success(), "premise: real git fast-forwards");
    assert!(
        mg_out.status.success(),
        "mg merge failed: {}",
        String::from_utf8_lossy(&mg_out.stderr)
    );

    assert_fast_forward_equal(&git_side, &mg_side, "fast-forward deleting the last file of a dir");
    assert!(
        !mg_side.join("gone").exists(),
        "the emptied directory must be pruned like real git does"
    );
    assert!(
        !mg_side.join("gone/only.txt").exists(),
        "the deleted file must be gone"
    );
    assert_eq!(
        fs::read_to_string(mg_side.join("dir/keep.txt")).expect("keep"),
        "keep v2\n"
    );
}

// ---------------------------------------------------------------------------
// 3. 快进新增多层目录 + 可执行位 + symlink
// ---------------------------------------------------------------------------

#[test]
fn fast_forward_adding_multilevel_dirs_and_modes_matches_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (git_side, mg_side) = two_copies(tmp.path(), "d1-add", |repo| {
        write_file(repo, "root.txt", "root\n");
        commit_all(repo, "base");
        git_ok(repo, &["checkout", "-q", "-b", "side"]);
        write_file(repo, "a/b/c/d.txt", "deep\n");
        write_file(repo, "a/b/e.txt", "mid\n");
        write_exec(repo, "a/b/c/run.sh", "#!/bin/sh\necho hi\n");
        std::os::unix::fs::symlink("a/b/c/d.txt", repo.join("link")).expect("symlink");
        commit_all(repo, "side adds deep dirs, an exec bit and a symlink");
        git_ok(repo, &["checkout", "-q", "main"]);
    });

    let git_out = git(&git_side, &["merge", "side"]);
    let mg_out = mg(&mg_side, &["merge", "side"]);
    assert!(git_out.status.success(), "premise: real git fast-forwards");
    assert!(
        mg_out.status.success(),
        "mg merge failed: {}",
        String::from_utf8_lossy(&mg_out.stderr)
    );

    assert_fast_forward_equal(&git_side, &mg_side, "fast-forward adding multilevel dirs");
    assert_eq!(
        fs::read_to_string(mg_side.join("a/b/c/d.txt")).expect("d.txt"),
        "deep\n"
    );
    assert_eq!(
        fs::metadata(mg_side.join("a/b/c/run.sh"))
            .expect("stat run.sh")
            .permissions()
            .mode()
            & 0o777,
        0o755,
        "the executable bit must survive the fast-forward"
    );
    assert_eq!(
        fs::read_link(mg_side.join("link")).expect("readlink"),
        PathBuf::from("a/b/c/d.txt"),
        "the symlink must be materialized as a symlink with the recorded target"
    );
}

// ---------------------------------------------------------------------------
// 4. 本地改动会丢 → 双方都必须拒绝，且工作区/index/refs 一字不动
// ---------------------------------------------------------------------------

#[test]
fn fast_forward_refuses_local_changes_it_would_overwrite() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (git_side, mg_side) = two_copies(tmp.path(), "d1-dirty", |repo| {
        write_file(repo, "dir/b.txt", "beta\n");
        write_file(repo, "only.txt", "only\n");
        commit_all(repo, "base");
        git_ok(repo, &["checkout", "-q", "-b", "side"]);
        write_file(repo, "dir/b.txt", "beta v2\n");
        write_file(repo, "s.txt", "s\n");
        commit_all(repo, "side edits dir/b.txt");
        git_ok(repo, &["checkout", "-q", "main"]);
    });

    // 两份副本上做**完全相同**的本地改动（真实 git 会拒绝的那种：会被快进覆盖）。
    for side in [&git_side, &mg_side] {
        write_file(side, "dir/b.txt", "local dirty\n");
    }
    let before_git = snapshot(&git_side);
    let before_mg = snapshot(&mg_side);

    let git_out = git(&git_side, &["merge", "side"]);
    let mg_out = mg(&mg_side, &["merge", "side"]);
    assert!(
        !git_out.status.success(),
        "premise broken: real git allowed the merge to overwrite a local change"
    );
    assert!(
        !mg_out.status.success(),
        "mg must refuse to overwrite local changes, got stdout={:?}",
        String::from_utf8_lossy(&mg_out.stdout)
    );

    assert_eq!(snapshot(&git_side), before_git, "git side must be untouched");
    assert_eq!(snapshot(&mg_side), before_mg, "mg side must be untouched");
    assert_eq!(porcelain(&mg_side), porcelain(&git_side), "porcelain after refusal");
    assert_eq!(
        fs::read_to_string(mg_side.join("dir/b.txt")).expect("dir/b.txt"),
        "local dirty\n",
        "the local content must survive"
    );
    assert!(
        !mg_side.join("s.txt").exists(),
        "nothing from the incoming commit may be materialized on refusal"
    );
    assert_both_fsck_clean(&git_side, &mg_side, "refused fast-forward");
}

// ---------------------------------------------------------------------------
// 5. 非快进回归：干净三方合并（子目录）
// ---------------------------------------------------------------------------

#[test]
fn non_fast_forward_clean_merge_with_subdirs_matches_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (git_side, mg_side) = two_copies(tmp.path(), "nonff-clean", |repo| {
        write_file(repo, "dir/b.txt", "beta\n");
        write_file(repo, "root.txt", "root\n");
        commit_all(repo, "base");
        git_ok(repo, &["checkout", "-q", "-b", "side"]);
        write_file(repo, "dir/side.txt", "from side\n");
        write_file(repo, "dir/b.txt", "beta side\n");
        commit_all(repo, "side");
        git_ok(repo, &["checkout", "-q", "main"]);
        write_file(repo, "root.txt", "root main\n");
        write_file(repo, "dir/main.txt", "from main\n");
        commit_all(repo, "main");
    });

    let base = head_oid(&git_side);
    let target = git_text(&git_side, &["rev-parse", "refs/heads/side"]);
    assert!(
        !git(&git_side, &["merge-base", "--is-ancestor", &base, &target])
            .status
            .success(),
        "premise broken: this is a fast-forward, not a three-way merge"
    );

    let git_out = git(&git_side, &["merge", "side"]);
    let mg_out = mg(&mg_side, &["merge", "side"]);
    assert!(git_out.status.success(), "premise: real git merges cleanly");
    assert!(
        mg_out.status.success(),
        "mg merge failed: {}",
        String::from_utf8_lossy(&mg_out.stderr)
    );

    assert_side_state_equal(&git_side, &mg_side, "clean three-way merge with subdirs");
    assert_eq!(
        parents(&mg_side),
        parents(&git_side),
        "the merge commit must have the same two parents as git's"
    );
    assert_eq!(
        parents(&mg_side).split_whitespace().count(),
        2,
        "a real merge commit has two parents"
    );
    for name in ["dir/side.txt", "dir/main.txt"] {
        assert_eq!(
            fs::read_to_string(git_side.join(name)).expect("git side file"),
            fs::read_to_string(mg_side.join(name)).expect("mg side file"),
            "{name} content must match"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. 非快进回归：子目录内冲突（冲突码、stage、字节），且真实 git 能收尾
// ---------------------------------------------------------------------------

#[test]
fn non_fast_forward_conflict_in_subdir_matches_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (git_side, mg_side) = two_copies(tmp.path(), "nonff-conflict", |repo| {
        write_file(repo, "dir/b.txt", "a\nb\nc\nd\ne\n");
        commit_all(repo, "base");
        git_ok(repo, &["checkout", "-q", "-b", "side"]);
        write_file(repo, "dir/b.txt", "a\nTHEIRS\nc\nd\ne\n");
        commit_all(repo, "side");
        git_ok(repo, &["checkout", "-q", "main"]);
        write_file(repo, "dir/b.txt", "a\nOURS\nc\nd\ne\n");
        commit_all(repo, "main");
    });

    let git_out = git(&git_side, &["merge", "side"]);
    let mg_out = mg(&mg_side, &["merge", "side"]);
    assert!(!git_out.status.success(), "premise: real git conflicts");
    assert!(
        !mg_out.status.success(),
        "mg must report the conflict with a non-zero exit code"
    );

    assert_eq!(porcelain(&mg_side), porcelain(&git_side), "conflict porcelain");
    assert_eq!(
        index_stage(&mg_side),
        index_stage(&git_side),
        "conflict index stages (1/2/3)"
    );
    assert_eq!(
        fs::read(git_side.join("dir/b.txt")).expect("git conflict bytes"),
        fs::read(mg_side.join("dir/b.txt")).expect("mg conflict bytes"),
        "conflict file bytes (markers) must match git byte for byte"
    );
    assert_eq!(manifest(&mg_side), manifest(&git_side), "conflict worktree manifest");
    assert!(
        mg_side.join(".git/MERGE_HEAD").is_file(),
        "mg must leave .git/MERGE_HEAD so real git can finish the merge"
    );
    assert_both_fsck_clean(&git_side, &mg_side, "conflicted merge");

    // 真实 git 必须能在 mg 留下的状态上直接完成合并。
    for side in [&git_side, &mg_side] {
        git_ok(side, &["add", "dir/b.txt"]);
        git_ok(side, &["commit", "-q", "-m", "resolved"]);
    }
    assert_eq!(
        parents(&mg_side).split_whitespace().count(),
        2,
        "real git must have created a merge commit on top of mg's state"
    );
    assert_side_state_equal(&git_side, &mg_side, "after git finishes mg's merge");
}

// ---------------------------------------------------------------------------
// 7. T15 的纯 mg 最小复现 + 真实 git 独立检视结果
// ---------------------------------------------------------------------------

#[test]
fn mg_native_d1_repro_is_clean_under_real_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).expect("mkdir");

    mg_ok(&repo, &["init", "."]);
    write_file(&repo, "dir/b.txt", "beta\n");
    mg_ok(&repo, &["add", "."]);
    mg_ok(&repo, &["commit", "-m", "c1"]);
    mg_ok(&repo, &["switch", "-c", "side", "main"]);
    write_file(&repo, "s.txt", "s\n");
    mg_ok(&repo, &["add", "s.txt"]);
    mg_ok(&repo, &["commit", "-m", "c2"]);
    mg_ok(&repo, &["switch", "main"]);

    let out = mg(&repo, &["merge", "side"]);
    assert!(
        out.status.success(),
        "T15's minimal repro must now succeed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // 真值：真实 git 对 mg 写出的仓库的判定。
    assert_eq!(
        head_oid(&repo),
        git_text(&repo, &["rev-parse", "refs/heads/side"]),
        "the branch must land exactly on side"
    );
    assert_eq!(porcelain(&repo), "", "git must see a clean worktree");
    assert_eq!(
        fs::read_to_string(repo.join("dir/b.txt")).expect("dir/b.txt"),
        "beta\n"
    );
    assert_eq!(fs::read_to_string(repo.join("s.txt")).expect("s.txt"), "s\n");
    assert_eq!(
        git_text(&repo, &["diff", "HEAD", "--name-only"]),
        "",
        "index and worktree must agree with HEAD"
    );
    assert_eq!(fsck_problems(&repo), Vec::<String>::new(), "git fsck");
}

// ---------------------------------------------------------------------------
// 8/9. `mg pull`：快进路径与非快进（分叉）路径
// ---------------------------------------------------------------------------

struct PullFixture {
    remote: PathBuf,
    seed: PathBuf,
    git_side: PathBuf,
    mg_side: PathBuf,
}

/// 远程（bare）+ 已推送的 `main`；两份**逐字节相同**的 clone（各带 origin 配置）。
fn pull_fixture<F>(tmp: &Path, label: &str, base_setup: F) -> PullFixture
where
    F: FnOnce(&Path),
{
    let remote = tmp.join(format!("{label}-remote.git"));
    git_ok(
        tmp,
        &["init", "-q", "--bare", "-b", "main", remote.to_str().expect("utf8")],
    );

    let seed = tmp.join(format!("{label}-seed"));
    fs::create_dir_all(&seed).expect("mkdir");
    git_ok(&seed, &["init", "-q", "-b", "main"]);
    base_setup(&seed);
    commit_all(&seed, "base");
    git_ok(&seed, &["remote", "add", "origin", remote.to_str().expect("utf8")]);
    git_ok(&seed, &["push", "-q", "-u", "origin", "main"]);

    let git_side = tmp.join(format!("{label}-git"));
    git_ok(
        tmp,
        &["clone", "-q", remote.to_str().expect("utf8"), git_side.to_str().expect("utf8")],
    );
    let mg_side = tmp.join(format!("{label}-mg"));
    copy_tree(&git_side, &mg_side);

    PullFixture {
        remote,
        seed,
        git_side,
        mg_side,
    }
}

fn advance_remote(fixture: &PullFixture, rel: &str, contents: &str, message: &str) {
    write_file(&fixture.seed, rel, contents);
    commit_all(&fixture.seed, message);
    git_ok(&fixture.seed, &["push", "-q", "origin", "main"]);
    let pushed = git_text(&fixture.seed, &["rev-parse", "HEAD"]);
    assert!(
        !pushed.is_empty(),
        "premise: the remote must have advanced ({} {})",
        fixture.remote.display(),
        message
    );
}

#[test]
fn pull_fast_forward_with_subdir_matches_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = pull_fixture(tmp.path(), "pull-ff", |seed| {
        write_file(seed, "dir/b.txt", "beta\n");
        write_file(seed, "root.txt", "root\n");
    });
    advance_remote(&fixture, "dir/b.txt", "beta v2\n", "remote advances dir/b.txt");

    // 前提自检：远端确实是本地 HEAD 的后代（即这是一次快进）。
    // 注意要在 **seed**（同时拥有两个对象）里判定：clone 在 fetch 之前还不认识远端新提交。
    let local_head = head_oid(&fixture.git_side);
    let remote_head = git_text(&fixture.seed, &["rev-parse", "HEAD"]);
    assert_eq!(
        ancestor_exit_code(&fixture.seed, &local_head, &remote_head),
        Some(0),
        "premise broken: the remote commit is not a descendant of the local HEAD"
    );

    let git_out = git(&fixture.git_side, &["-c", "pull.rebase=false", "pull", "--no-edit"]);
    let mg_out = mg(&fixture.mg_side, &["pull"]);
    assert!(
        git_out.status.success(),
        "premise: real git pull fast-forwards: {}",
        String::from_utf8_lossy(&git_out.stderr)
    );
    assert!(
        mg_out.status.success(),
        "mg pull failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&mg_out.stdout),
        String::from_utf8_lossy(&mg_out.stderr)
    );

    assert_fast_forward_equal(&fixture.git_side, &fixture.mg_side, "pull fast-forward");
    assert_eq!(
        head_oid(&fixture.mg_side),
        git_text(&fixture.seed, &["rev-parse", "HEAD"]),
        "mg pull must land on the remote commit"
    );
    assert_eq!(
        fs::read_to_string(fixture.mg_side.join("dir/b.txt")).expect("dir/b.txt"),
        "beta v2\n"
    );
}

#[test]
fn pull_when_local_is_ahead_changes_nothing_like_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = pull_fixture(tmp.path(), "pull-ahead", |seed| {
        write_file(seed, "dir/b.txt", "beta\n");
    });

    // 两份副本上做**完全相同**的本地前进提交，然后都不推。
    for side in [&fixture.git_side, &fixture.mg_side] {
        write_file(side, "dir/local.txt", "local\n");
        commit_all(side, "local ahead");
    }
    let before_git = snapshot(&fixture.git_side);
    let before_mg = snapshot(&fixture.mg_side);

    let git_out = git(&fixture.git_side, &["-c", "pull.rebase=false", "pull", "--no-edit"]);
    let mg_out = mg(&fixture.mg_side, &["pull"]);
    assert!(
        git_out.status.success(),
        "premise: real git pull is a no-op here: {}",
        String::from_utf8_lossy(&git_out.stderr)
    );
    assert!(
        mg_out.status.success(),
        "mg pull failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&mg_out.stdout),
        String::from_utf8_lossy(&mg_out.stderr)
    );
    assert_eq!(snapshot(&fixture.git_side), before_git, "git side changed");
    assert_eq!(snapshot(&fixture.mg_side), before_mg, "mg side changed");
    assert_fast_forward_equal(&fixture.git_side, &fixture.mg_side, "pull when local is ahead");
}

#[test]
fn pull_divergent_branches_merges_like_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = pull_fixture(tmp.path(), "pull-diverge", |seed| {
        write_file(seed, "dir/b.txt", "beta\n");
        write_file(seed, "root.txt", "root\n");
    });

    // 本地先分叉：在一份**独立**的副本上提交，再复制成两份，保证两边起点逐字节相同。
    let local = tmp.path().join("pull-diverge-local");
    copy_tree(&fixture.mg_side, &local);
    write_file(&local, "dir/local.txt", "local\n");
    commit_all(&local, "local divergence");
    fs::remove_dir_all(&fixture.mg_side).expect("rm pristine mg clone");
    copy_tree(&local, &fixture.mg_side);
    fs::remove_dir_all(&fixture.git_side).expect("rm pristine git clone");
    copy_tree(&local, &fixture.git_side);

    advance_remote(&fixture, "dir/remote.txt", "remote\n", "remote divergence");

    let local_head = head_oid(&fixture.git_side);
    let remote_head = git_text(&fixture.seed, &["rev-parse", "HEAD"]);
    // fetch 之后 clone 才认识远端新提交；退出码 1（而不是 128）才说明「不是祖先」。
    git_ok(&fixture.git_side, &["fetch", "-q", "origin"]);
    assert_eq!(
        ancestor_exit_code(&fixture.git_side, &local_head, &remote_head),
        Some(1),
        "premise broken: local is not diverged from the remote"
    );

    let git_out = git(&fixture.git_side, &["-c", "pull.rebase=false", "pull", "--no-edit"]);
    let mg_out = mg(&fixture.mg_side, &["pull"]);
    assert!(
        git_out.status.success(),
        "premise: real git pull merges: {}",
        String::from_utf8_lossy(&git_out.stderr)
    );
    assert!(
        mg_out.status.success(),
        "mg pull failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&mg_out.stdout),
        String::from_utf8_lossy(&mg_out.stderr)
    );

    assert_side_state_equal(&fixture.git_side, &fixture.mg_side, "divergent pull merge");
    assert_eq!(
        parents(&fixture.mg_side),
        parents(&fixture.git_side),
        "merge parents after a divergent pull"
    );
    assert_eq!(parents(&fixture.mg_side).split_whitespace().count(), 2);
    // 两侧都必须真正把远端的文件带进工作区。
    assert_eq!(
        fs::read_to_string(fixture.mg_side.join("dir/remote.txt")).expect("remote.txt"),
        "remote\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.mg_side.join("dir/local.txt")).expect("local.txt"),
        "local\n"
    );
}

#[test]
fn pull_divergent_conflict_matches_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let fixture = pull_fixture(tmp.path(), "pull-conflict", |seed| {
        write_file(seed, "dir/b.txt", "a\nb\nc\nd\ne\n");
    });

    let local = tmp.path().join("pull-conflict-local");
    copy_tree(&fixture.mg_side, &local);
    write_file(&local, "dir/b.txt", "a\nLOCAL\nc\nd\ne\n");
    commit_all(&local, "local divergence");
    fs::remove_dir_all(&fixture.mg_side).expect("rm pristine mg clone");
    copy_tree(&local, &fixture.mg_side);
    fs::remove_dir_all(&fixture.git_side).expect("rm pristine git clone");
    copy_tree(&local, &fixture.git_side);

    advance_remote(&fixture, "dir/b.txt", "a\nREMOTE\nc\nd\ne\n", "remote divergence");

    let git_out = git(&fixture.git_side, &["-c", "pull.rebase=false", "pull", "--no-edit"]);
    let mg_out = mg(&fixture.mg_side, &["pull"]);
    assert!(
        !git_out.status.success(),
        "premise: real git pull conflicts here: {}",
        String::from_utf8_lossy(&git_out.stderr)
    );
    assert!(
        !mg_out.status.success(),
        "mg pull must surface the conflict with a non-zero exit code, stdout={:?}",
        String::from_utf8_lossy(&mg_out.stdout)
    );

    assert_eq!(porcelain(&fixture.mg_side), porcelain(&fixture.git_side), "pull conflict porcelain");
    assert_eq!(
        index_stage(&fixture.mg_side),
        index_stage(&fixture.git_side),
        "pull conflict index stages"
    );
    let git_bytes = fs::read(fixture.git_side.join("dir/b.txt")).expect("git bytes");
    let mg_bytes = fs::read(fixture.mg_side.join("dir/b.txt")).expect("mg bytes");
    assert_eq!(
        normalize_conflict_labels(&mg_bytes),
        normalize_conflict_labels(&git_bytes),
        "pull conflict body (labels normalized)"
    );
    if mg_bytes != git_bytes {
        println!(
            "V16 OBSERVATION: `mg pull` conflict label differs from `git pull`: mg={:?} git={:?} \
             (matches `git merge <ref>` instead)",
            theirs_label(&mg_bytes),
            theirs_label(&git_bytes)
        );
    }
    assert_eq!(
        manifest(&fixture.mg_side),
        manifest(&fixture.git_side),
        "pull conflict worktree manifest"
    );
    assert_side_state_equal(&fixture.git_side, &fixture.mg_side, "divergent pull conflict");
}
