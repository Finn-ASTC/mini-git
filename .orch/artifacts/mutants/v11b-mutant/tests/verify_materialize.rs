//! V11 —— 独立验证 T11（tree 物化 + `branch`/`switch`/`checkout`/`reset`）。
//!
//! 纪律：
//!
//! * **真值来源 = 运行时的真实 `git` 进程 + 文件系统**：`git ls-files --stage`、
//!   `git status --porcelain`、`git rev-parse HEAD`、`git for-each-ref`、`.git/HEAD` 字节。
//!   期望值零硬编码，也不把「解析 git 输出再自己重算」当成真值。
//! * **平行仓库对拍**：每个场景 `cp -a` 出 A（跑 `mg`）与 B（跑真实 `git`），跑同样的
//!   命令序列，再逐字节比较工作区（含可执行位 / symlink 目标 / 空文件 / 非 UTF-8 文件名）、
//!   index（`ls-files --stage`）、`status --porcelain`、`rev-parse HEAD`、`.git/HEAD`、refs。
//! * **不许假绿**：被测对象既走 `mg` 二进制（`env!("CARGO_BIN_EXE_mg")`），也直接调
//!   `minigit::worktree::materialize::{materialize_tree, write_blob_to}`。
//!
//! 场景 8（symlink 祖先）按 controller-correction.md v2 的**夹具矩阵**断言（V11b 复验后
//! 重写）：删除路径遇 symlink 祖先绝不越界（非 force 拒绝 / force 放行，symlink 均保留）；
//! 写路径仅当「目标已在 index 且顺 symlink `stat` 不到」时才换成工作区内真目录，
//! `stat` 得到或路径未跟踪时拒绝。矩阵由本轮用 `git version 2.55.0` 当场跑出（23/23 格一致）。
#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use minigit::error::Error;
use minigit::object::{FileMode, Kind, Tree};
use minigit::odb::Odb;
use minigit::oid::Oid;
use minigit::refs::RefStore;
use minigit::repo::Repo;
use minigit::worktree::materialize::{materialize_tree, write_blob_to};
use minigit::worktree::MaterializeOptions;

const MG: &str = env!("CARGO_BIN_EXE_mg");

// ------------------------------------------------------------------ 进程封装

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn git_base(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .env("HOME", dir)
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "v11 verify")
        .env("GIT_AUTHOR_EMAIL", "v11@example.invalid")
        .env("GIT_COMMITTER_NAME", "v11 verify")
        .env("GIT_COMMITTER_EMAIL", "v11@example.invalid")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000");
    cmd
}

fn git_raw(dir: &Path, args: &[&str]) -> Output {
    git_base(dir).args(args).output().expect("spawn git")
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let out = git_raw(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} 失败\nstdout: {}\nstderr: {}",
        lossy(&out.stdout),
        lossy(&out.stderr)
    );
    out
}

fn git_out(dir: &Path, args: &[&str]) -> Vec<u8> {
    git(dir, args).stdout
}

fn mg_raw(dir: &Path, args: &[&str]) -> Output {
    Command::new(MG)
        .current_dir(dir)
        .args(args)
        .env("HOME", dir)
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("spawn mg")
}

fn assert_no_panic(out: &Output, what: &str) {
    let err = lossy(&out.stderr);
    assert!(!err.contains("panicked at"), "{what}: 不许 panic，stderr=\n{err}");
    assert_ne!(out.status.code(), Some(101), "{what}: exit 101 = Rust panic，stderr=\n{err}");
}

fn mg(dir: &Path, args: &[&str]) -> Output {
    let out = mg_raw(dir, args);
    assert!(
        out.status.success(),
        "mg {args:?} 失败\nstdout: {}\nstderr: {}",
        lossy(&out.stdout),
        lossy(&out.stderr)
    );
    out
}

fn mg_fails(dir: &Path, args: &[&str]) -> Output {
    let out = mg_raw(dir, args);
    assert!(
        !out.status.success(),
        "mg {args:?} 竟然成功了\nstdout: {}\nstderr: {}",
        lossy(&out.stdout),
        lossy(&out.stderr)
    );
    assert_no_panic(&out, &format!("mg {args:?}"));
    out
}

// ------------------------------------------------------------------ 文件系统

fn write_file(dir: &Path, rel: &[u8], bytes: &[u8], exec: bool) {
    let path = dir.join(OsStr::from_bytes(rel));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir -p");
    }
    fs::write(&path, bytes).expect("write");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(if exec { 0o755 } else { 0o644 });
    fs::set_permissions(&path, perms).expect("chmod");
}

fn write_link(dir: &Path, rel: &[u8], target: &[u8]) {
    let path = dir.join(OsStr::from_bytes(rel));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir -p");
    }
    symlink(OsStr::from_bytes(target), &path).expect("symlink");
}

fn read(dir: &Path, rel: &[u8]) -> Option<Vec<u8>> {
    fs::read(dir.join(OsStr::from_bytes(rel))).ok()
}

fn exists(dir: &Path, rel: &[u8]) -> bool {
    fs::symlink_metadata(dir.join(OsStr::from_bytes(rel))).is_ok()
}

fn is_exec(dir: &Path, rel: &[u8]) -> bool {
    fs::metadata(dir.join(OsStr::from_bytes(rel)))
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn cp_a(src: &Path, dst: &Path) {
    let out = Command::new("cp")
        .arg("-a")
        .arg(src)
        .arg(dst)
        .output()
        .expect("spawn cp");
    assert!(
        out.status.success(),
        "cp -a {src:?} {dst:?} 失败: {}",
        lossy(&out.stderr)
    );
}

fn head_hex(dir: &Path) -> String {
    lossy(&git_out(dir, &["rev-parse", "HEAD"])).trim().to_string()
}

// ------------------------------------------------------------------ 工作区快照

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Dir,
    File { bytes: Vec<u8>, exec: bool },
    Link { target: Vec<u8> },
}

fn work_snapshot(root: &Path) -> BTreeMap<Vec<u8>, Node> {
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<Vec<u8>, Node>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .expect("read_dir")
        .map(|entry| entry.expect("dir entry"))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .expect("strip_prefix")
            .as_os_str()
            .as_bytes()
            .to_vec();
        if rel == b".git" {
            continue;
        }
        let file_type = entry.file_type().expect("file type");
        if file_type.is_symlink() {
            let target = fs::read_link(&path).expect("read_link");
            out.insert(
                rel,
                Node::Link {
                    target: target.as_os_str().as_bytes().to_vec(),
                },
            );
        } else if file_type.is_dir() {
            out.insert(rel, Node::Dir);
            walk(root, &path, out);
        } else {
            let meta = entry.metadata().expect("metadata");
            out.insert(
                rel,
                Node::File {
                    bytes: fs::read(&path).expect("read"),
                    exec: meta.permissions().mode() & 0o111 != 0,
                },
            );
        }
    }
}

fn esc(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &byte in bytes {
        match byte {
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out
}

fn render(map: &BTreeMap<Vec<u8>, Node>) -> String {
    let mut out = String::new();
    for (path, node) in map {
        match node {
            Node::Dir => out.push_str(&format!("dir      {}\n", esc(path))),
            Node::Link { target } => {
                out.push_str(&format!("symlink  {} -> {}\n", esc(path), esc(target)));
            }
            Node::File { bytes, exec } => {
                let shown = if bytes.len() > 60 {
                    format!("{}...", esc(&bytes[..60]))
                } else {
                    esc(bytes)
                };
                out.push_str(&format!(
                    "file{}   {} ({} bytes) {}\n",
                    if *exec { "*" } else { " " },
                    esc(path),
                    bytes.len(),
                    shown
                ));
            }
        }
    }
    if out.is_empty() {
        out.push_str("(empty)\n");
    }
    out
}

fn dump(dir: &Path) -> String {
    render(&work_snapshot(dir))
}

/// 逐文件 `sha256sum` 清单（外加 symlink 目标 / 目录），用于证明「工作区一字未动」。
fn sha256_manifest(root: &Path) -> String {
    let snap = work_snapshot(root);
    let files: Vec<&Vec<u8>> = snap
        .iter()
        .filter(|(_, node)| matches!(node, Node::File { .. }))
        .map(|(path, _)| path)
        .collect();
    let mut lines: Vec<String> = Vec::new();
    if !files.is_empty() {
        let mut cmd = Command::new("sha256sum");
        cmd.current_dir(root).arg("--");
        for file in &files {
            cmd.arg(OsStr::from_bytes(file));
        }
        let out = cmd.output().expect("spawn sha256sum");
        assert!(out.status.success(), "sha256sum 失败: {}", lossy(&out.stderr));
        for line in out.stdout.split(|byte| *byte == b'\n') {
            if !line.is_empty() {
                lines.push(lossy(line));
            }
        }
    }
    for (path, node) in &snap {
        match node {
            Node::Link { target } => {
                lines.push(format!("symlink {} -> {}", esc(path), esc(target)));
            }
            Node::Dir => lines.push(format!("dir {}", esc(path))),
            Node::File { .. } => {}
        }
    }
    lines.join("\n")
}

// ------------------------------------------------------------------ 平行仓库

struct Pair {
    tmp: tempfile::TempDir,
    parent: PathBuf,
    a: PathBuf,
    b: PathBuf,
}

impl Pair {
    /// 建一个父仓库（真值仓库），后面 `fork()` 出 A/B 两份。
    fn new() -> Pair {
        let tmp = tempfile::tempdir().expect("tempdir");
        let parent = tmp.path().join("parent");
        fs::create_dir_all(&parent).expect("mkdir parent");
        git(&parent, &["init", "-q", "-b", "main"]);
        Pair {
            tmp,
            parent: parent.clone(),
            a: parent.clone(),
            b: parent.clone(),
        }
    }

    fn p(&self) -> &Path {
        &self.parent
    }

    fn a(&self) -> &Path {
        &self.a
    }

    fn b(&self) -> &Path {
        &self.b
    }

    fn fork(&mut self) {
        let a = self.tmp.path().join("A");
        let b = self.tmp.path().join("B");
        cp_a(&self.parent, &a);
        cp_a(&self.parent, &b);
        self.a = a;
        self.b = b;
    }

    /// A（mg）与 B（git）的五项状态逐字节比较 + refs。
    fn compare(&self, label: &str) {
        compare_state(&self.a, &self.b, label);
    }
}

fn compare_state(a: &Path, b: &Path, label: &str) {
    let wa = dump(a);
    let wb = dump(b);
    assert_eq!(wa, wb, "[{label}] 工作区不一致（左=A/mg，右=B/git）");

    let ia = lossy(&git_out(a, &["ls-files", "--stage"]));
    let ib = lossy(&git_out(b, &["ls-files", "--stage"]));
    assert_eq!(ia, ib, "[{label}] git ls-files --stage 不一致\nA={ia}\nB={ib}");

    let sa = lossy(&git_out(a, &["status", "--porcelain"]));
    let sb = lossy(&git_out(b, &["status", "--porcelain"]));
    assert_eq!(sa, sb, "[{label}] git status --porcelain 不一致\nA={sa:?}\nB={sb:?}");

    let ha = lossy(&git_out(a, &["rev-parse", "HEAD"]));
    let hb = lossy(&git_out(b, &["rev-parse", "HEAD"]));
    assert_eq!(ha, hb, "[{label}] git rev-parse HEAD 不一致");

    let ba = fs::read(a.join(".git/HEAD")).expect("read HEAD (A)");
    let bb = fs::read(b.join(".git/HEAD")).expect("read HEAD (B)");
    assert_eq!(esc(&ba), esc(&bb), "[{label}] .git/HEAD 不一致");

    compare_refs(a, b, label);
}

fn compare_refs(a: &Path, b: &Path, label: &str) {
    let ra = lossy(&git_out(a, &["for-each-ref", "--format=%(refname) %(objectname)"]));
    let rb = lossy(&git_out(b, &["for-each-ref", "--format=%(refname) %(objectname)"]));
    assert_eq!(ra, rb, "[{label}] refs 不一致\nA={ra}\nB={rb}");
}

fn compare_orig_head(a: &Path, b: &Path, label: &str) {
    let oa = fs::read(a.join(".git/ORIG_HEAD")).expect("ORIG_HEAD (A)");
    let ob = fs::read(b.join(".git/ORIG_HEAD")).expect("ORIG_HEAD (B)");
    assert_eq!(esc(&oa), esc(&ob), "[{label}] .git/ORIG_HEAD 不一致");
    assert_eq!(oa.len(), 41, "[{label}] ORIG_HEAD 必须是 40 hex + 换行");
    assert_eq!(oa[40], b'\n', "[{label}] ORIG_HEAD 必须以换行结尾");
    assert!(ob.len() == 41 && ob[40] == b'\n');
    assert!(oa[..40].iter().all(u8::is_ascii_hexdigit), "[{label}] ORIG_HEAD 必须是十六进制");
    assert!(ob[..40].iter().all(u8::is_ascii_hexdigit));
}

// ------------------------------------------------------------------ 库 API 辅助

fn open_repo(dir: &Path) -> Repo {
    Repo::discover(dir).expect("Repo::discover")
}

fn tree_of(dir: &Path, rev: &str) -> Tree {
    let repo = open_repo(dir);
    let hex = lossy(&git_out(dir, &["rev-parse", &format!("{rev}^{{tree}}")]));
    let oid = Oid::from_hex(hex.trim()).expect("tree oid");
    Odb::new(&repo)
        .read_object(oid)
        .expect("read tree object")
        .into_tree()
        .expect("into_tree")
}

fn oid_of(dir: &Path, spec: &str) -> Oid {
    let hex = lossy(&git_out(dir, &["rev-parse", spec]));
    Oid::from_hex(hex.trim()).expect("oid")
}

fn current_branch_names(dir: &Path) -> BTreeSet<String> {
    lossy(&git_out(
        dir,
        &["branch", "--list", "--format=%(refname:short)"],
    ))
    .lines()
    .map(str::to_string)
    .collect()
}

fn current_branch_names_prefix(dir: &Path, pattern: &str) -> BTreeSet<String> {
    lossy(&git_out(
        dir,
        &["branch", "--list", pattern, "--format=%(refname:short)"],
    ))
    .lines()
    .map(str::to_string)
    .collect()
}

fn mg_branch_names(dir: &Path, args: &[&str]) -> BTreeSet<String> {
    let out = mg(dir, args);
    lossy(&out.stdout)
        .lines()
        .map(|line| line.trim_start_matches(['*', ' ']).to_string())
        .filter(|name| !name.is_empty())
        .collect()
}

// ------------------------------------------------------------------ 场景 1

/// 场景 1：普通分支切换（多目录 / 可执行位 / symlink / 空文件 / 非 UTF-8 文件名）。
#[test]
fn switch_branch_matches_git_on_rich_worktree() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"dir1/a.txt", b"alpha\n", false);
    write_file(&p, b"dir2/sub/b.bin", &[0, 1, 2, 3, 250, 251, 252, 253], false);
    write_file(&p, b"exec.sh", b"#!/bin/sh\necho hi\n", true);
    write_file(&p, b"empty", b"", false);
    write_file(&p, b"weird\xff\xfe.txt", b"weird\n", false);
    write_link(&p, b"link", b"dir1/a.txt");
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "base"]);

    git(&p, &["switch", "-q", "-c", "other"]);
    write_file(&p, b"dir1/a.txt", b"beta\n", false);
    fs::remove_file(p.join("empty")).unwrap();
    write_file(&p, b"new/deep/file.txt", b"deep\n", false);
    write_file(&p, b"exec.sh", b"#!/bin/sh\necho hi\n", false);
    fs::remove_file(p.join("link")).unwrap();
    write_link(&p, b"link", b"dir2/sub/b.bin");
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "second"]);
    git(&p, &["switch", "-q", "main"]);

    pair.fork();
    pair.compare("fork 基线（A=mg / B=git，尚未跑命令）");

    mg(pair.a(), &["switch", "other"]);
    git(pair.b(), &["switch", "-q", "other"]);
    pair.compare("switch other");
    // 场景自检：这次切换确实造出了「非 UTF-8 名 / 空文件消失 / 可执行位变化 / symlink 换靶」
    assert_eq!(read(pair.a(), b"dir1/a.txt").as_deref(), Some(&b"beta\n"[..]));
    assert_eq!(
        read(pair.a(), b"weird\xff\xfe.txt").as_deref(),
        Some(&b"weird\n"[..])
    );
    assert!(!exists(pair.a(), b"empty"), "rev2 删掉的空文件必须消失");
    assert!(!is_exec(pair.a(), b"exec.sh"), "rev2 里 exec.sh 不可执行");
    assert_eq!(
        fs::read_link(pair.a().join("link")).unwrap(),
        PathBuf::from("dir2/sub/b.bin")
    );

    mg(pair.a(), &["switch", "main"]);
    git(pair.b(), &["switch", "-q", "main"]);
    pair.compare("switch 回 main");
    assert!(is_exec(pair.a(), b"exec.sh"), "main 里 exec.sh 必须恢复可执行位");
    assert!(exists(pair.a(), b"empty"));
    assert_eq!(
        fs::read_link(pair.a().join("link")).unwrap(),
        PathBuf::from("dir1/a.txt")
    );
}

// ------------------------------------------------------------------ 场景 2

/// 场景 2：`switch -c`（建 → 切回 → 再切走），带一个两分支同内容的本地改动。
#[test]
fn switch_create_roundtrip_matches_git() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"base\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "base"]);
    write_file(&p, b"g.txt", b"second\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "second"]);

    pair.fork();
    for dir in [pair.a().to_path_buf(), pair.b().to_path_buf()] {
        write_file(&dir, b"f.txt", b"dirty but same on both branches\n", false);
    }

    mg(pair.a(), &["switch", "-c", "topic", "main"]);
    git(pair.b(), &["switch", "-q", "-c", "topic", "main"]);
    pair.compare("switch -c topic main");
    assert_eq!(
        read(pair.a(), b"f.txt").as_deref(),
        Some(&b"dirty but same on both branches\n"[..]),
        "两分支同内容的本地改动必须被保留"
    );

    mg(pair.a(), &["switch", "main"]);
    git(pair.b(), &["switch", "-q", "main"]);
    pair.compare("switch main");

    mg(pair.a(), &["switch", "topic"]);
    git(pair.b(), &["switch", "-q", "topic"]);
    pair.compare("switch topic（再切走）");
}

// ------------------------------------------------------------------ 场景 3

/// 场景 3：detached checkout（40 位 oid + `--detach`）。
#[test]
fn checkout_detached_matches_git() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"one\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    let rev1 = head_hex(&p);
    write_file(&p, b"f.txt", b"two\n", false);
    write_file(&p, b"g.txt", b"g\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "two"]);

    pair.fork();
    mg(pair.a(), &["checkout", "--detach", &rev1]);
    git(pair.b(), &["checkout", "-q", "--detach", &rev1]);
    pair.compare("checkout --detach <rev1>");
    assert!(!exists(pair.a(), b"g.txt"));
    let head = fs::read(pair.a().join(".git/HEAD")).unwrap();
    assert_eq!(head.len(), 41, "detached .git/HEAD 必须是 40 hex + 换行");
    assert_eq!(lossy(&head).trim(), rev1);

    mg(pair.a(), &["checkout", "main"]);
    git(pair.b(), &["checkout", "-q", "main"]);
    pair.compare("checkout main（重新 attach）");
    assert_eq!(
        lossy(&fs::read(pair.a().join(".git/HEAD")).unwrap()),
        "ref: refs/heads/main\n"
    );
}

// ------------------------------------------------------------------ 场景 4

/// 场景 4：`checkout <rev> -- <path>`（单文件 / 目录路径 / 未命中 pathspec）。
#[test]
fn checkout_paths_matches_git() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"one\n", false);
    write_file(&p, b"dir/x.txt", b"x1\n", false);
    write_file(&p, b"dir/y.txt", b"y1\n", false);
    write_file(&p, b"other.txt", b"o1\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    let rev1 = head_hex(&p);
    write_file(&p, b"f.txt", b"two\n", false);
    write_file(&p, b"dir/x.txt", b"x2\n", false);
    write_file(&p, b"other.txt", b"o2\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "two"]);

    pair.fork();
    mg(pair.a(), &["checkout", &rev1, "--", "f.txt"]);
    git(pair.b(), &["checkout", "-q", &rev1, "--", "f.txt"]);
    pair.compare("checkout <rev1> -- f.txt");
    assert_eq!(
        read(pair.a(), b"other.txt").as_deref(),
        Some(&b"o2\n"[..]),
        "未被 pathspec 命中的路径必须一字不动"
    );
    assert_eq!(read(pair.a(), b"dir/x.txt").as_deref(), Some(&b"x2\n"[..]));

    mg(pair.a(), &["checkout", &rev1, "--", "dir"]);
    git(pair.b(), &["checkout", "-q", &rev1, "--", "dir"]);
    pair.compare("checkout <rev1> -- dir");
    assert_eq!(read(pair.a(), b"dir/y.txt").as_deref(), Some(&b"y1\n"[..]));
    assert_eq!(read(pair.a(), b"other.txt").as_deref(), Some(&b"o2\n"[..]));

    let out = mg_fails(pair.a(), &["checkout", &rev1, "--", "nosuch.txt"]);
    assert!(
        lossy(&out.stderr).contains("did not match"),
        "stderr={}",
        lossy(&out.stderr)
    );
    assert!(
        !git_raw(pair.b(), &["checkout", &rev1, "--", "nosuch.txt"]).status.success(),
        "真实 git 同样拒绝不存在的 pathspec"
    );
}

// ------------------------------------------------------------------ 场景 5

fn reset_fixture() -> (Pair, String) {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"one\n", false);
    write_file(&p, b"keep.txt", b"k\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    let rev1 = head_hex(&p);
    write_file(&p, b"f.txt", b"two\n", false);
    write_file(&p, b"g.txt", b"g\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "two"]);
    pair.fork();
    for dir in [pair.a().to_path_buf(), pair.b().to_path_buf()] {
        write_file(&dir, b"f.txt", b"local edit\n", false);
        write_file(&dir, b"staged.txt", b"staged\n", false);
        git(&dir, &["add", "staged.txt"]);
        write_file(&dir, b"untracked.txt", b"u\n", false);
    }
    (pair, rev1)
}

/// 场景 5a：`reset --soft`（有本地改动 + 暂存 + 未跟踪）。
#[test]
fn reset_soft_matches_git_with_local_changes() {
    let (pair, rev1) = reset_fixture();
    let before = dump(pair.a());
    mg(pair.a(), &["reset", "--soft", &rev1]);
    git(pair.b(), &["reset", "-q", "--soft", &rev1]);
    pair.compare("reset --soft");
    compare_orig_head(pair.a(), pair.b(), "reset --soft");
    assert_eq!(before, dump(pair.a()), "reset --soft 不许动工作区");
    assert_eq!(read(pair.a(), b"f.txt").as_deref(), Some(&b"local edit\n"[..]));
    assert!(exists(pair.a(), b"untracked.txt"));
    assert_eq!(head_hex(pair.a()), rev1);
}

/// 场景 5b：`reset --mixed`（index 重建，工作区不动）。
#[test]
fn reset_mixed_matches_git_with_local_changes() {
    let (pair, rev1) = reset_fixture();
    let before = dump(pair.a());
    mg(pair.a(), &["reset", "--mixed", &rev1]);
    git(pair.b(), &["reset", "-q", "--mixed", &rev1]);
    pair.compare("reset --mixed");
    compare_orig_head(pair.a(), pair.b(), "reset --mixed");
    assert_eq!(before, dump(pair.a()), "reset --mixed 不许动工作区");
    assert_eq!(read(pair.a(), b"f.txt").as_deref(), Some(&b"local edit\n"[..]));
    assert!(exists(pair.a(), b"staged.txt"), "工作区里的暂存文件不受 --mixed 影响");
    let staged = lossy(&git_out(pair.a(), &["ls-files", "--stage"]));
    assert!(!staged.contains("staged.txt"), "index 应已换成 rev1 的 tree：{staged}");
    assert_eq!(head_hex(pair.a()), rev1);
}

/// 场景 5c：`reset --hard`（工作区也被物化，未跟踪文件保留）。
#[test]
fn reset_hard_matches_git_with_local_changes() {
    let (pair, rev1) = reset_fixture();
    mg(pair.a(), &["reset", "--hard", &rev1]);
    git(pair.b(), &["reset", "-q", "--hard", &rev1]);
    pair.compare("reset --hard");
    compare_orig_head(pair.a(), pair.b(), "reset --hard");
    assert_eq!(read(pair.a(), b"f.txt").as_deref(), Some(&b"one\n"[..]));
    assert!(!exists(pair.a(), b"g.txt"));
    assert!(exists(pair.a(), b"untracked.txt"), "reset --hard 必须保留未跟踪文件");
}

/// 场景 5d：干净工作区上 `reset --hard` → `status --porcelain` 必须为空。
#[test]
fn reset_hard_on_clean_worktree_leaves_clean_status() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"one\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    let rev1 = head_hex(&p);
    write_file(&p, b"f.txt", b"two\n", false);
    write_file(&p, b"g.txt", b"g\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "two"]);

    pair.fork();
    mg(pair.a(), &["reset", "--hard", &rev1]);
    git(pair.b(), &["reset", "-q", "--hard", &rev1]);
    pair.compare("reset --hard（干净工作区）");
    assert_eq!(
        lossy(&git_out(pair.a(), &["status", "--porcelain"])),
        "",
        "reset --hard 后 status --porcelain 必须为空"
    );
    assert_eq!(read(pair.a(), b"f.txt").as_deref(), Some(&b"one\n"[..]));
    assert!(!exists(pair.a(), b"g.txt"));
}

// ------------------------------------------------------------------ 场景 6

/// 场景 6：会被覆盖的本地改动必须拒绝；`sha256sum` 证明工作区一字未动。
#[test]
fn dirty_local_change_is_refused_and_worktree_untouched() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"one\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    git(&p, &["switch", "-q", "-c", "other"]);
    write_file(&p, b"f.txt", b"other\n", false);
    write_file(&p, b"extra.txt", b"extra\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "other"]);
    git(&p, &["switch", "-q", "main"]);

    pair.fork();
    for dir in [pair.a().to_path_buf(), pair.b().to_path_buf()] {
        write_file(&dir, b"f.txt", b"local dirty\n", false);
    }
    let snap_before = dump(pair.a());
    let sha_before = sha256_manifest(pair.a());
    let index_before = fs::read(pair.a().join(".git/index")).unwrap();
    let head_before = fs::read(pair.a().join(".git/HEAD")).unwrap();
    let refs_before = lossy(&git_out(
        pair.a(),
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    ));

    for args in [vec!["switch", "other"], vec!["checkout", "other"]] {
        let out = mg_fails(pair.a(), &args);
        assert!(
            lossy(&out.stderr).contains("refusing to overwrite local changes"),
            "mg {args:?} stderr={}",
            lossy(&out.stderr)
        );
    }
    assert!(
        !git_raw(pair.b(), &["switch", "other"]).status.success(),
        "真实 git 也必须拒绝 switch"
    );
    assert!(
        !git_raw(pair.b(), &["checkout", "other"]).status.success(),
        "真实 git 也必须拒绝 checkout"
    );

    assert_eq!(snap_before, dump(pair.a()), "拒绝后工作区必须一字未动");
    assert_eq!(sha_before, sha256_manifest(pair.a()), "sha256sum 清单必须完全相同");
    assert_eq!(
        index_before,
        fs::read(pair.a().join(".git/index")).unwrap(),
        "拒绝后 index 不应被改写"
    );
    assert_eq!(head_before, fs::read(pair.a().join(".git/HEAD")).unwrap());
    assert_eq!(
        refs_before,
        lossy(&git_out(
            pair.a(),
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        ))
    );

    // 直接调库：错误类型必须是 WouldLoseChanges，且同样不动物化
    let repo = open_repo(pair.a());
    let tree = tree_of(pair.a(), "other");
    let err = materialize_tree(&repo, &tree, &MaterializeOptions { force: false })
        .expect_err("有会被覆盖的本地改动时必须拒绝");
    assert!(matches!(err, Error::WouldLoseChanges(_)), "实际错误 {err:?}");
    drop(repo);
    assert_eq!(
        sha_before,
        sha256_manifest(pair.a()),
        "库调用被拒后工作区仍必须一字未动"
    );
}

// ------------------------------------------------------------------ 场景 7

/// 场景 7：删除语义 —— rev2 删掉的已跟踪文件必须消失，未跟踪 / ignored 文件必须还在。
#[test]
fn deletion_semantics_keeps_untracked_and_ignored() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b".gitignore", b"ignored.txt\nignored_dir/\n", false);
    write_file(&p, b"keep.txt", b"keep\n", false);
    write_file(&p, b"gone.txt", b"gone\n", false);
    write_file(&p, b"dir/gone2.txt", b"gone2\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    git(&p, &["switch", "-q", "-c", "prune"]);
    fs::remove_file(p.join("gone.txt")).unwrap();
    fs::remove_file(p.join("dir/gone2.txt")).unwrap();
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "prune"]);
    git(&p, &["switch", "-q", "main"]);

    pair.fork();
    for dir in [pair.a().to_path_buf(), pair.b().to_path_buf()] {
        write_file(&dir, b"untracked.txt", b"u\n", false);
        write_file(&dir, b"ignored.txt", b"i\n", false);
        write_file(&dir, b"ignored_dir/junk.txt", b"j\n", false);
    }
    // 场景自检：ignored 判定由真实 git 给出（porcelain 里只该出现 untracked.txt）
    assert_eq!(
        lossy(&git_out(pair.a(), &["status", "--porcelain"])),
        "?? untracked.txt\n"
    );

    mg(pair.a(), &["switch", "prune"]);
    git(pair.b(), &["switch", "-q", "prune"]);
    pair.compare("switch 到删掉文件的 rev");

    assert!(!exists(pair.a(), b"gone.txt"), "rev2 删掉的已跟踪文件必须消失");
    assert!(!exists(pair.a(), b"dir/gone2.txt"));
    assert!(!exists(pair.a(), b"dir"), "空掉的目录必须被清掉");
    assert!(exists(pair.a(), b"keep.txt"));
    assert!(exists(pair.a(), b"untracked.txt"), "未跟踪文件必须还在");
    assert!(exists(pair.a(), b"ignored.txt"), "ignored 文件必须还在");
    assert!(
        exists(pair.a(), b"ignored_dir/junk.txt"),
        "ignored 目录里的文件必须还在"
    );
}

// ------------------------------------------------------------------ 场景 8

// 真值口径 = `controller-correction.md`（v2）的**夹具矩阵**，本轮由真实 `git version 2.55.0`
// 自跑复现（原始矩阵 `/tmp/v11b/matrix/full.log`，23/23 格 mg 与 git 一致）。规则不是
// 「遇 symlink 祖先一律拒绝」，而是取决于顺 symlink 能否 `stat` 到目标：
//   * 写路径 + 目标已在 index 且顺 symlink `stat` 不到（ENOENT）→ 放行，把 symlink 换成
//     工作区内的真目录；
//   * 写路径 + 顺 symlink `stat` 得到 → 视为本地改动会被覆盖 → 拒绝；
//   * 写路径 + 未跟踪的新路径 → 一律拒绝；
//   * 删除路径 → 绝不跟随 symlink：非 force 拒绝（exit 1、symlink 保留），force 放行
//     （exit 0、symlink 保留、工作区外零变动，`prune_empty_dirs` 不得 rmdir 其目标）。
// 库级 `write_blob_to()`（内部唯一父目录检查 `ensure_parents` 非 pub）必须一律拒绝。

/// 源仓库：main(`a/b/c`=v1、`real/b/c`=v1、`keep.txt`) / other(把 `a/b/c` 改 v2) /
/// add(新增 `a/new.txt`) / prune(删 `a/b/c`)。
fn build_symlink_source(parent: &Path) {
    write_file(parent, b"a/b/c", b"v1\n", false);
    write_file(parent, b"real/b/c", b"v1\n", false);
    write_file(parent, b"keep.txt", b"k\n", false);
    git(parent, &["add", "-A"]);
    git(parent, &["commit", "-qm", "one"]);
    git(parent, &["switch", "-q", "-c", "other"]);
    write_file(parent, b"a/b/c", b"v2\n", false);
    git(parent, &["add", "-A"]);
    git(parent, &["commit", "-qm", "two"]);
    git(parent, &["switch", "-q", "main"]);
    git(parent, &["switch", "-q", "-c", "add"]);
    write_file(parent, b"a/new.txt", b"new\n", false);
    git(parent, &["add", "-A"]);
    git(parent, &["commit", "-qm", "add"]);
    git(parent, &["switch", "-q", "main"]);
    git(parent, &["switch", "-q", "-c", "prune"]);
    git(parent, &["rm", "-q", "a/b/c"]);
    git(parent, &["commit", "-qm", "prune"]);
    git(parent, &["switch", "-q", "main"]);
}

/// 删掉真 `a/`、把 `a` 换成指向 `outside` 的 symlink；`outside_present=true` 时在 outside
/// 里放 `b/c`（内容与 index 相同），使顺 symlink 祖先 `stat` 得到目标。
fn plant_symlink_ancestor(dir: &Path, outside: &Path, outside_present: bool) {
    if outside_present {
        fs::create_dir_all(outside.join("b")).unwrap();
        fs::write(outside.join("b/c"), b"v1\n").unwrap();
    } else {
        fs::create_dir_all(outside).unwrap();
    }
    fs::remove_dir_all(dir.join("a")).unwrap();
    symlink(outside, dir.join("a")).unwrap();
}

/// 造 A(mg)/B(git) 平行仓库 + symlink 祖先夹具（两侧 outside 路径不同）。
fn symlink_pair(outside_present: bool) -> (Pair, PathBuf, PathBuf) {
    let mut pair = Pair::new();
    build_symlink_source(pair.p());
    pair.fork();
    let out_a = pair.tmp.path().join("A-outside");
    let out_b = pair.tmp.path().join("B-outside");
    plant_symlink_ancestor(pair.a(), &out_a, outside_present);
    plant_symlink_ancestor(pair.b(), &out_b, outside_present);
    (pair, out_a, out_b)
}

fn is_symlink(dir: &Path, rel: &str) -> bool {
    fs::symlink_metadata(dir.join(rel))
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn is_dir(dir: &Path, rel: &str) -> bool {
    !is_symlink(dir, rel) && dir.join(rel).is_dir()
}

/// 工作区 + index (`ls-files --stage`) + status + HEAD + refs + `.git/HEAD`，
/// 已按 `subs` 把两侧不同的绝对路径归一化。
fn state_text(dir: &Path, subs: &[(&Path, &str)]) -> String {
    let mut text = format!(
        "== worktree ==\n{}\n== stage ==\n{}== status ==\n{}== HEAD ==\n{}== refs ==\n{}== .git/HEAD ==\n{}",
        dump(dir),
        lossy(&git_out(dir, &["ls-files", "--stage"])),
        lossy(&git_out(dir, &["status", "--porcelain"])),
        lossy(&git_out(dir, &["rev-parse", "HEAD"])),
        lossy(&git_out(dir, &["for-each-ref", "--format=%(refname) %(objectname)"])),
        esc(&fs::read(dir.join(".git/HEAD")).expect(".git/HEAD")),
    );
    for (path, repl) in subs {
        text = text.replace(&path.to_string_lossy().to_string(), repl);
    }
    text
}

fn compare_state_normalized(
    a: &Path,
    b: &Path,
    a_subs: &[(&Path, &str)],
    b_subs: &[(&Path, &str)],
    label: &str,
) {
    let ta = state_text(a, a_subs);
    let tb = state_text(b, b_subs);
    assert_eq!(ta, tb, "[{label}] A(mg) 与 B(git) 状态不一致\nA={ta}\nB={tb}");
}

/// 工作区外路径逐字节比较前的归一化包装（两侧 outside / 仓库根绝对路径不同）。
fn compare_outside_pair(a: &Path, b: &Path, out_a: &Path, out_b: &Path, label: &str) {
    compare_state_normalized(
        a,
        b,
        &[(out_a, "OUTSIDE"), (a, "ROOT")],
        &[(out_b, "OUTSIDE"), (b, "ROOT")],
        label,
    );
}

/// 场景 8.1（写 + outside 为空）：顺 symlink stat 不到目标 → mg 与 git 都必须 exit 0，
/// symlink 被换成工作区内的真目录，工作区外逐字节不变。
#[test]
fn symlink_write_outside_empty_replaces_link_matches_git() {
    let (pair, out_a, out_b) = symlink_pair(false);
    let before_a = sha256_manifest(&out_a);
    let before_b = sha256_manifest(&out_b);

    // 前提自证：真实 git 在同一夹具上放行（前提若变，这里先失败）
    git(pair.b(), &["switch", "other"]);
    mg(pair.a(), &["switch", "other"]);

    compare_outside_pair(pair.a(), pair.b(), &out_a, &out_b, "写/outside-empty");
    assert_eq!(sha256_manifest(&out_a), before_a, "mg 写到了工作区外");
    assert_eq!(sha256_manifest(&out_b), before_b, "git 写到了工作区外");
    assert!(!is_symlink(pair.a(), "a"), "mg 必须把 symlink 祖先换成真目录");
    assert!(is_dir(pair.a(), "a"), "换出来的必须是真目录");
    let canon_root = fs::canonicalize(pair.a()).unwrap();
    let canon = fs::canonicalize(pair.a().join("a/b/c")).unwrap();
    assert!(
        canon.starts_with(&canon_root),
        "canonicalize(a/b/c)={canon:?} 落到工作区外"
    );
    assert_eq!(read(pair.a(), b"a/b/c").as_deref(), Some(&b"v2\n"[..]));
    assert_eq!(
        fs::read_dir(&out_a).unwrap().count(),
        0,
        "outside 本为空夹具，mg 不得往里写任何东西"
    );
}

/// 场景 8.2（写 + outside 里目标存在）：顺 symlink stat 得到目标 → mg 与 git 都必须拒绝，
/// 工作区 / index / 工作区外零改动。
#[test]
fn symlink_write_outside_present_refuses_without_touching_anything() {
    let (pair, out_a, out_b) = symlink_pair(true);
    let work_before = dump(pair.a());
    let outside_before = sha256_manifest(&out_a);
    let status_before = lossy(&git_out(pair.a(), &["status", "--porcelain"]));
    let index_before = fs::read(pair.a().join(".git/index")).unwrap();
    let head_before = fs::read(pair.a().join(".git/HEAD")).unwrap();

    let mg_run = mg_raw(pair.a(), &["switch", "other"]);
    let git_run = git_raw(pair.b(), &["switch", "other"]);

    assert!(
        !git_run.status.success(),
        "前提自证失败：真实 git 竟然放行（stderr={}）",
        lossy(&git_run.stderr)
    );
    assert!(
        !mg_run.status.success(),
        "mg 必须像 git 一样拒绝（exit={:?} stderr={}）",
        mg_run.status.code(),
        lossy(&mg_run.stderr)
    );
    assert_no_panic(&mg_run, "mg switch other");
    assert!(
        lossy(&mg_run.stderr).contains("refusing to overwrite"),
        "mg 拒绝理由异常：{}",
        lossy(&mg_run.stderr)
    );

    assert_eq!(dump(pair.a()), work_before, "拒绝时工作区必须一字未动");
    assert_eq!(sha256_manifest(&out_a), outside_before, "拒绝时工作区外必须一字未动");
    assert_eq!(
        fs::read(pair.a().join(".git/index")).unwrap(),
        index_before,
        "拒绝时 index 必须一字未动"
    );
    assert_eq!(fs::read(pair.a().join(".git/HEAD")).unwrap(), head_before);
    assert!(is_symlink(pair.a(), "a"), "拒绝后 symlink 祖先必须保留");
    // 下面这句之后才调 git（git status 会刷新 index stat 缓存，不能拿来当「index 未变」的基线）
    assert_eq!(
        lossy(&git_out(pair.a(), &["status", "--porcelain"])),
        status_before
    );
    compare_outside_pair(pair.a(), pair.b(), &out_a, &out_b, "写/outside-present");
}

/// 场景 8.3（写 + 未跟踪的新路径）：`a/new.txt` 不在 main 的 index 里 → mg 与 git 都拒绝，
/// symlink 不得被替换。
#[test]
fn symlink_write_untracked_new_path_refuses_like_git() {
    let (pair, out_a, out_b) = symlink_pair(false);
    let work_before = dump(pair.a());
    let outside_before = sha256_manifest(&out_a);
    let status_before = lossy(&git_out(pair.a(), &["status", "--porcelain"]));
    let index_before = fs::read(pair.a().join(".git/index")).unwrap();

    let mg_run = mg_raw(pair.a(), &["switch", "add"]);
    let git_run = git_raw(pair.b(), &["switch", "add"]);

    assert!(
        !git_run.status.success(),
        "前提自证失败：真实 git 竟然放行（stderr={}）",
        lossy(&git_run.stderr)
    );
    // 任务书此处写作 `... would be overwritten: a`；git 2.55.0 实测输出是下面这句（含 `by checkout`），
    // 以实测为准（见 result 的「真值来源自证」一节）。
    assert!(
        lossy(&git_run.stderr).contains("untracked working tree files would be overwritten"),
        "git 真值输出有变，需重新自证：{}",
        lossy(&git_run.stderr)
    );
    assert!(
        !mg_run.status.success(),
        "mg 必须拒绝未跟踪的新路径（stderr={}）",
        lossy(&mg_run.stderr)
    );
    assert_no_panic(&mg_run, "mg switch add");
    assert!(
        lossy(&mg_run.stderr).contains("refusing to overwrite"),
        "mg 拒绝理由异常：{}",
        lossy(&mg_run.stderr)
    );

    assert_eq!(dump(pair.a()), work_before, "拒绝时工作区必须一字未动");
    assert_eq!(sha256_manifest(&out_a), outside_before, "拒绝时工作区外必须一字未动");
    assert_eq!(fs::read(pair.a().join(".git/index")).unwrap(), index_before);
    assert!(is_symlink(pair.a(), "a"), "拒绝后 symlink 祖先必须保留");
    assert_eq!(
        lossy(&git_out(pair.a(), &["status", "--porcelain"])),
        status_before
    );
    compare_outside_pair(pair.a(), pair.b(), &out_a, &out_b, "写/untracked-new-path");
}

/// 删除路径 + symlink 祖先（工作区外有数据）的单格：非 force 拒绝、force 放行，
/// 两档都不得跟随 symlink 动到工作区外（`sha256sum` 清单逐字节证明），且 symlink 必须保留。
fn symlink_removal_case(label: &str, mg_args: &[&str], git_args: &[&str], expect_success: bool) {
    let (pair, out_a, out_b) = symlink_pair(true);
    let before_a = sha256_manifest(&out_a);
    let before_b = sha256_manifest(&out_b);

    let mg_run = mg_raw(pair.a(), mg_args);
    let git_run = git_raw(pair.b(), git_args);

    assert_eq!(
        git_run.status.success(),
        expect_success,
        "[{label}] 前提自证失败：git exit={:?} stderr={}",
        git_run.status.code(),
        lossy(&git_run.stderr)
    );
    assert_eq!(
        mg_run.status.success(),
        expect_success,
        "[{label}] mg exit={:?} 与 git 不一致 stderr={}",
        mg_run.status.code(),
        lossy(&mg_run.stderr)
    );
    assert_no_panic(&mg_run, label);

    assert_eq!(sha256_manifest(&out_a), before_a, "[{label}] mg 动了工作区外");
    assert_eq!(sha256_manifest(&out_b), before_b, "[{label}] git 动了工作区外");
    assert!(is_symlink(pair.a(), "a"), "[{label}] mg 把 symlink 祖先删了/换了");
    assert!(is_symlink(pair.b(), "a"), "[{label}] git 把 symlink 祖先删了/换了");
    // prune_empty_dirs 不得 rmdir 掉 symlink 指向的 outside（其 b/c 必须原样）
    assert_eq!(
        read(&out_a, b"b/c").as_deref(),
        Some(&b"v1\n"[..]),
        "[{label}] mg 删到了工作区外"
    );
    assert_eq!(
        read(&out_b, b"b/c").as_deref(),
        Some(&b"v1\n"[..]),
        "[{label}] git 删到了工作区外"
    );
    compare_outside_pair(pair.a(), pair.b(), &out_a, &out_b, label);
    if expect_success {
        let st = lossy(&git_out(pair.a(), &["status", "--porcelain"]));
        assert_eq!(st, "?? a\n", "[{label}] status={st:?}");
        assert_eq!(read(pair.a(), b"real/b/c").as_deref(), Some(&b"v1\n"[..]));
    } else {
        assert!(
            lossy(&mg_run.stderr).contains("refusing to overwrite"),
            "[{label}] mg 拒绝理由异常：{}",
            lossy(&mg_run.stderr)
        );
    }
}

/// 场景 8.4：删除路径遇 symlink 祖先（outside 有数据），三个命令 × 两档 force 与真实 git
/// 逐格一致 —— 非 force `switch`/`checkout` exit 1、symlink 保留；`switch -f` / `checkout -f`
/// / `reset --hard` exit 0、symlink 保留、outside 零变动、status 只剩 `?? a`。
#[test]
fn symlink_ancestor_removal_never_touches_outside() {
    symlink_removal_case(
        "switch prune 非 force",
        &["switch", "prune"],
        &["switch", "prune"],
        false,
    );
    symlink_removal_case(
        "checkout prune 非 force",
        &["checkout", "prune"],
        &["checkout", "prune"],
        false,
    );
    symlink_removal_case(
        "switch -f prune",
        &["switch", "-f", "prune"],
        &["switch", "-f", "prune"],
        true,
    );
    symlink_removal_case(
        "checkout -f prune",
        &["checkout", "-f", "prune"],
        &["checkout", "-f", "prune"],
        true,
    );
    symlink_removal_case(
        "reset --hard prune",
        &["reset", "--hard", "prune"],
        &["reset", "--hard", "prune"],
        true,
    );
}

/// symlink 祖先指向**工作区内**目录（`a -> real`）的单格对拍。
fn inside_symlink_case(
    label: &str,
    mg_args: &[&str],
    git_args: &[&str],
    expect_success: bool,
    expect_link: bool,
) {
    let mut pair = Pair::new();
    build_symlink_source(pair.p());
    pair.fork();
    for dir in [pair.a().to_path_buf(), pair.b().to_path_buf()] {
        fs::remove_dir_all(dir.join("a")).unwrap();
        symlink(Path::new("real"), dir.join("a")).unwrap();
    }

    let mg_run = mg_raw(pair.a(), mg_args);
    let git_run = git_raw(pair.b(), git_args);
    assert_eq!(
        git_run.status.success(),
        expect_success,
        "[{label}] 前提自证失败：git exit={:?} stderr={}",
        git_run.status.code(),
        lossy(&git_run.stderr)
    );
    assert_eq!(
        mg_run.status.success(),
        expect_success,
        "[{label}] mg exit={:?} 与 git 不一致 stderr={}",
        mg_run.status.code(),
        lossy(&mg_run.stderr)
    );
    assert_no_panic(&mg_run, label);
    assert_eq!(is_symlink(pair.a(), "a"), expect_link, "[{label}] mg 的 a 形态不对");
    assert_eq!(is_symlink(pair.b(), "a"), expect_link, "[{label}] git 的 a 形态不对");
    // 无论哪一格，工作区内的 symlink 目标 real/b/c 都不许被删/改
    assert_eq!(
        read(pair.a(), b"real/b/c").as_deref(),
        Some(&b"v1\n"[..]),
        "[{label}] mg 动了 real/b/c"
    );
    assert_eq!(
        read(pair.b(), b"real/b/c").as_deref(),
        Some(&b"v1\n"[..]),
        "[{label}] git 动了 real/b/c"
    );
    compare_state_normalized(pair.a(), pair.b(), &[], &[], label);
    if expect_success && !expect_link {
        assert_eq!(
            read(pair.a(), b"a/b/c").as_deref(),
            Some(&b"v2\n"[..]),
            "[{label}] 换真目录后内容不对"
        );
    }
    if !expect_success {
        assert!(
            lossy(&mg_run.stderr).contains("refusing to overwrite"),
            "[{label}] stderr={}",
            lossy(&mg_run.stderr)
        );
    }
}

/// 场景 8.5（symlink 祖先指向工作区内目录）：删除与写入各一例，与真实 git 逐字节对拍。
#[test]
fn symlink_ancestor_inside_worktree_matches_git() {
    inside_symlink_case(
        "inside/switch prune 非 force",
        &["switch", "prune"],
        &["switch", "prune"],
        false,
        true,
    );
    inside_symlink_case(
        "inside/switch -f prune",
        &["switch", "-f", "prune"],
        &["switch", "-f", "prune"],
        true,
        true,
    );
    inside_symlink_case(
        "inside/switch -f other",
        &["switch", "-f", "other"],
        &["switch", "-f", "other"],
        true,
        false,
    );
}

/// 场景 8.6（库级守卫不得回退）：直接调 `write_blob_to()` —— 其唯一父目录检查
/// `ensure_parents` 是私有函数，集成测试只能经这个 pub 入口覆盖；symlink 祖先必须返回
/// `Err(WouldLoseChanges)`，不得跟随 symlink 写出、不得把 symlink 换成真目录。
#[test]
fn library_write_guard_still_refuses_symlink_ancestor() {
    let pair = Pair::new();
    build_symlink_source(pair.p());
    let p = pair.p().to_path_buf();
    let outside = pair.tmp.path().join("lib-outside");
    plant_symlink_ancestor(&p, &outside, false);
    let before = sha256_manifest(&outside);

    let repo = open_repo(&p);
    let blob = oid_of(&p, "other:a/b/c");
    let err = write_blob_to(&repo, b"a/b/c", blob, FileMode::Regular)
        .expect_err("祖先 symlink 时必须拒绝写出");
    assert!(matches!(err, Error::WouldLoseChanges(_)), "实际错误 {err:?}");
    drop(repo);

    assert!(is_symlink(&p, "a"), "库级写不得把 symlink 祖先换成真目录");
    assert_eq!(sha256_manifest(&outside), before, "库级写碰到了工作区外");
    assert!(
        fs::read_dir(&outside).unwrap().next().is_none(),
        "工作区外被写入了内容"
    );
    assert!(!exists(&p, b"a/b/c"), "库级写不得在 symlink 祖先下写出目标");
}

// ------------------------------------------------------------------ 场景 9

/// 场景 9：不存在的分支 / rev / oid → 对应错误类型；不许 panic、不许留半成品。
#[test]
fn negative_cases_are_typed_and_leave_no_half_state() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"one\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    git(&p, &["branch", "exists"]);
    pair.fork();

    let snap_before = dump(pair.a());
    let head_before = fs::read(pair.a().join(".git/HEAD")).unwrap();
    // 先让 git status 刷新一次 index（它会重写 stat 缓存 / cache-tree），再把 index 字节当基线，
    // 否则会把自己触发的 index 改写误判成 mg 留下半成品。
    let status_before = lossy(&git_out(pair.a(), &["status", "--porcelain"]));
    let index_before = fs::read(pair.a().join(".git/index")).unwrap();
    let refs_before = lossy(&git_out(
        pair.a(),
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    ));

    let missing = "1111111111111111111111111111111111111111";
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["switch", "nosuchbranch"], "reference not found"),
        (vec!["checkout", "nosuchrev"], "reference not found"),
        (vec!["checkout", missing], "object not found"),
        (vec!["reset", "--hard", "nosuchrev"], "reference not found"),
        (
            vec!["switch", "-c", "newbranch", "nosuchstart"],
            "reference not found",
        ),
        (vec!["switch", "-c", "exists", "main"], "already exists"),
        (vec!["branch", "exists", "main"], "already exists"),
    ];
    for (args, want) in &cases {
        let out = mg_fails(pair.a(), args);
        assert!(
            lossy(&out.stderr).contains(want),
            "mg {args:?} 的错误应含 {want:?}，实际 stderr={}",
            lossy(&out.stderr)
        );
    }

    assert_eq!(snap_before, dump(pair.a()), "反例不许留半成品（工作区）");
    assert_eq!(
        index_before,
        fs::read(pair.a().join(".git/index")).unwrap(),
        "反例不许留半成品（index）"
    );
    assert_eq!(head_before, fs::read(pair.a().join(".git/HEAD")).unwrap());
    assert_eq!(
        status_before,
        lossy(&git_out(pair.a(), &["status", "--porcelain"]))
    );
    assert!(
        !exists(pair.a(), b".git/refs/heads/newbranch"),
        "失败的 switch -c 不许留下新分支"
    );
    assert_eq!(
        refs_before,
        lossy(&git_out(
            pair.a(),
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        ))
    );

    // 直接调库的错误类型
    let repo = open_repo(pair.a());
    let err = RefStore::new(&repo)
        .resolve("nosuchbranch")
        .expect_err("必须 RefNotFound");
    assert!(matches!(err, Error::RefNotFound(_)), "实际 {err:?}");
    let missing_oid = Oid::from_hex(missing).unwrap();
    let err = Odb::new(&repo)
        .read_object(missing_oid)
        .expect_err("必须 ObjectNotFound");
    assert!(matches!(err, Error::ObjectNotFound(_)), "实际 {err:?}");
}

// ------------------------------------------------------------------ 场景 10

/// 场景 10：直接调 `materialize_tree`，返回值必须是「本次删除的仓库相对路径」升序，
/// 且工作区结果与 `git reset --hard` 逐字节一致。
#[test]
fn materialize_tree_lib_api_returns_removed_paths_sorted() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"a.txt", b"a1\n", false);
    write_file(&p, b"b.txt", b"b1\n", false);
    write_file(&p, b"c.txt", b"c1\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    let rev1 = head_hex(&p);
    git(&p, &["switch", "-q", "-c", "two"]);
    fs::remove_file(p.join("a.txt")).unwrap();
    fs::remove_file(p.join("c.txt")).unwrap();
    write_file(&p, b"d.txt", b"d2\n", false);
    write_file(&p, b"e.txt", b"e2\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "two"]);

    pair.fork();
    // B 侧用真实 git 给出「commit1 的树」应有的工作区
    git(pair.b(), &["reset", "-q", "--hard", &rev1]);

    let repo = open_repo(pair.a());
    let odb = Odb::new(&repo);
    let tree_oid = oid_of(pair.a(), &format!("{rev1}^{{tree}}"));
    let tree = odb
        .read_object(tree_oid)
        .expect("read tree")
        .into_tree()
        .expect("into_tree");
    let removed = materialize_tree(&repo, &tree, &MaterializeOptions { force: true })
        .expect("materialize_tree 必须成功");

    assert_eq!(
        removed,
        vec![b"d.txt".to_vec(), b"e.txt".to_vec()],
        "返回值必须是「本次删除的仓库相对路径」升序"
    );
    assert_eq!(
        dump(pair.a()),
        dump(pair.b()),
        "物化结果必须等于 git reset --hard rev1"
    );
    assert_eq!(read(pair.a(), b"a.txt").as_deref(), Some(&b"a1\n"[..]));
    assert!(!exists(pair.a(), b"d.txt"));
}

/// 场景 10（续）：`write_blob_to` 单路径写出（普通 / 可执行 / symlink 三种 mode）。
#[test]
fn write_blob_to_lib_api_writes_single_path() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"one\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    pair.fork();

    let repo = open_repo(pair.a());
    let blob = oid_of(pair.a(), "main:f.txt");
    write_blob_to(&repo, b"sub/new.txt", blob, FileMode::Regular).expect("写普通文件");
    assert_eq!(read(pair.a(), b"sub/new.txt").as_deref(), Some(&b"one\n"[..]));
    assert!(!is_exec(pair.a(), b"sub/new.txt"));

    write_blob_to(&repo, b"sub/exec.sh", blob, FileMode::Executable).expect("写可执行文件");
    assert!(is_exec(pair.a(), b"sub/exec.sh"));

    let odb = Odb::new(&repo);
    let link_blob = odb.write(Kind::Blob, b"sub/new.txt").expect("write blob");
    write_blob_to(&repo, b"sub/link", link_blob, FileMode::Symlink).expect("写 symlink");
    assert_eq!(
        fs::read_link(pair.a().join("sub/link")).unwrap(),
        PathBuf::from("sub/new.txt")
    );
}

// ------------------------------------------------------------------ 场景 11

/// 场景 11：`mg branch` 的列表 / 建 / 删 / 重命名与真实 git 对拍。
#[test]
fn branch_commands_match_git() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"one\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    let c1 = head_hex(&p);
    write_file(&p, b"f.txt", b"two\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "two"]);
    git(&p, &["branch", "behind", &c1]);
    git(&p, &["switch", "-q", "-c", "ahead"]);
    write_file(&p, b"g.txt", b"g\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "three"]);
    git(&p, &["switch", "-q", "main"]);

    pair.fork();

    // 列表：集合必须等于 git 的 short name 集合，当前分支恰好一个 `* ` 标记
    assert_eq!(
        mg_branch_names(pair.a(), &["branch"]),
        current_branch_names(pair.b())
    );
    let listing = lossy(&mg(pair.a(), &["branch"]).stdout);
    let starred: Vec<&str> = listing
        .lines()
        .filter(|line| line.starts_with("* "))
        .collect();
    assert_eq!(starred.len(), 1, "必须恰好一个当前分支标记：{listing}");
    assert_eq!(starred[0].trim_start_matches("* "), "main");
    compare_refs(pair.a(), pair.b(), "branch 列表基线");

    // `-l <pattern>` 的集合也要与 git 一致
    assert_eq!(
        mg_branch_names(pair.a(), &["branch", "-l", "a*"]),
        current_branch_names_prefix(pair.b(), "a*")
    );

    // 建分支（只动 refs，不碰工作区）
    mg(pair.a(), &["branch", "newb", "main"]);
    git(pair.b(), &["branch", "newb", "main"]);
    compare_refs(pair.a(), pair.b(), "branch newb main");
    pair.compare("branch newb main");

    // 删已合并分支
    mg(pair.a(), &["branch", "-d", "newb"]);
    git(pair.b(), &["branch", "-d", "newb"]);
    compare_refs(pair.a(), pair.b(), "branch -d newb");
    mg(pair.a(), &["branch", "-d", "behind"]);
    git(pair.b(), &["branch", "-d", "behind"]);
    compare_refs(pair.a(), pair.b(), "branch -d behind（已合并）");

    // 删未合并分支：两边都拒绝，refs 不动
    let refs_before = lossy(&git_out(
        pair.a(),
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    ));
    let out = mg_fails(pair.a(), &["branch", "-d", "ahead"]);
    assert!(
        lossy(&out.stderr).contains("not fully merged"),
        "stderr={}",
        lossy(&out.stderr)
    );
    assert!(
        !git_raw(pair.b(), &["branch", "-d", "ahead"]).status.success(),
        "真实 git 也拒绝删未合并分支"
    );
    assert_eq!(
        refs_before,
        lossy(&git_out(
            pair.a(),
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        ))
    );

    // 删当前分支：两边都拒绝
    mg_fails(pair.a(), &["branch", "-d", "main"]);
    assert!(!git_raw(pair.b(), &["branch", "-d", "main"]).status.success());

    // 重命名当前分支（改 refs + HEAD 的 symbolic ref）
    mg(pair.a(), &["branch", "-m", "renamed"]);
    git(pair.b(), &["branch", "-m", "renamed"]);
    compare_refs(pair.a(), pair.b(), "branch -m renamed（当前分支）");
    pair.compare("branch -m renamed");

    // 重命名其它分支
    mg(pair.a(), &["branch", "-m", "ahead", "ahead2"]);
    git(pair.b(), &["branch", "-m", "ahead", "ahead2"]);
    compare_refs(pair.a(), pair.b(), "branch -m ahead ahead2");
}

// ------------------------------------------------------------------ 场景 12

/// 场景 12：真值来源自证（W2 P11 教训）—— `git ls-files --stage` / `git status --porcelain`
/// 是纯文本而不是 pkt-line，`.git/HEAD` 是裸字节。
#[test]
fn truth_sources_are_plain_text_not_pktline() {
    let mut pair = Pair::new();
    let p = pair.p().to_path_buf();
    write_file(&p, b"f.txt", b"one\n", false);
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-qm", "one"]);
    pair.fork();

    let stage = git_out(pair.a(), &["ls-files", "--stage"]);
    let text = lossy(&stage);
    assert!(text.contains('\t'), "ls-files --stage 用 TAB 分隔路径：{text:?}");
    assert!(
        !stage.contains(&0),
        "ls-files --stage 里不该有 NUL（pkt-line 不是这里的真值格式）"
    );
    let first = text.lines().next().expect("至少一行");
    let (meta, path) = first.split_once('\t').expect("mode oid stage\\tpath");
    let fields: Vec<&str> = meta.split_whitespace().collect();
    assert_eq!(fields.len(), 3, "`<mode> <oid> <stage>`，实际 {fields:?}");
    assert_eq!(fields[0].len(), 6, "mode 是 6 位八进制");
    assert_eq!(fields[1].len(), 40, "oid 是 40 hex");
    assert_eq!(fields[2], "0", "普通条目 stage=0");
    assert_eq!(path, "f.txt");

    write_file(pair.a(), b"new.txt", b"n\n", false);
    assert_eq!(
        lossy(&git_out(pair.a(), &["status", "--porcelain"])),
        "?? new.txt\n",
        "porcelain 是可解析的纯文本"
    );

    git(pair.a(), &["checkout", "-q", "--detach", "HEAD"]);
    let detached = fs::read(pair.a().join(".git/HEAD")).unwrap();
    assert_eq!(detached.len(), 41, "detached .git/HEAD = 40 hex + 换行");
    assert_eq!(detached[40], b'\n');
    assert!(detached[..40].iter().all(u8::is_ascii_hexdigit));
}
