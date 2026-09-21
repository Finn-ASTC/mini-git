//! V12 —— T12（add / rm / status / commit / log / tag + `T` 类型变化）的**独立**验证。
//!
//! 本文件由验证者（kind: opencode）编写，只做「真值对拍」：每一条期望值都在运行时
//! 由真实 `git`（2.55）子进程现场产出，文件里**不写死任何 git 的输出字节**。
//! 唯一固定的输入是用来让 git 自己可复现的提交时间戳（`FIXED_DATE`）。
//!
//! 覆盖：
//! * `add` / `rm` / `rm --cached` / `status --porcelain` / `commit` / `log --oneline` / `tag`
//!   与真实 git 的**进程输出**逐字节比较（≥3 个含分叉、合并、二进制、非 ASCII 路径的仓库）；
//! * `T` 类型变化（普通文件 ↔ symlink，工作区列与 index 列）；
//! * 双向互操作（mg 写的仓库真 git 能接着用；git 写的仓库 mg 能读）；
//! * 反例（`rm` 无 `-f` 拒绝且零改动、冲突状态下 `commit` 拒绝、`add` 不存在路径响亮失败）；
//! * 直接使用 `minigit::{index,refs,worktree,odb}` 交叉校验，避免「只比 CLI」的假绿。
//!
//! 真值来源自证（W2 教训 P11）：任务书 §2.2 举的 `git rm --cached f && ln -s target f`
//! **不产生 `T`**（实测 git 2.55 输出 `D  f` + `?? f`，且 `ln -s` 还会因工作区文件仍在而失败）。
//! 真正的 `T` 需要「index 仍跟踪该路径、工作区换成另一种类型」，见 `type_change_*` 用例。

#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use minigit::index::Index;
use minigit::object::{FileMode, Object, Tree, TreeEntry};
use minigit::odb::Odb;
use minigit::oid::Oid;
use minigit::refs::RefStore;
use minigit::repo::Repo;
use minigit::worktree::Worktree;

/// cargo 在集成测试里注入的 mg 二进制路径（必须真的调它，不能只调库）。
const MG: &str = env!("CARGO_BIN_EXE_mg");

/// 固定的提交时间戳：让「真实 git」自己的准备提交可复现（不是被测真值）。
const FIXED_DATE: i64 = 1_700_000_000;

// ===========================================================================
// 沙箱与进程工具
// ===========================================================================

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        Sandbox {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn join(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn write_bytes(&self, rel: &str, bytes: &[u8]) {
        let path = self.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(path, bytes).expect("write file");
    }

    fn write_str(&self, rel: &str, text: &str) {
        self.write_bytes(rel, text.as_bytes());
    }

    fn read(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.join(rel)).expect("read file")
    }

    fn exists(&self, rel: &str) -> bool {
        self.join(rel).symlink_metadata().is_ok()
    }

    fn remove(&self, rel: &str) {
        std::fs::remove_file(self.join(rel)).expect("remove file");
    }

    fn open_repo(&self) -> Repo {
        Repo::discover(self.path()).expect("discover mini-git repo")
    }

    // ---------------------------------------------------------------- git

    fn git_cmd(&self, args: &[&str], envs: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new("git");
        cmd.current_dir(self.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", self.path())
            .env("LC_ALL", "C")
            .env("GIT_AUTHOR_NAME", "Scenario Author")
            .env("GIT_AUTHOR_EMAIL", "author@example.com")
            .env("GIT_COMMITTER_NAME", "Scenario Author")
            .env("GIT_COMMITTER_EMAIL", "author@example.com")
            .env("GIT_AUTHOR_DATE", format!("{FIXED_DATE} +0800"))
            .env("GIT_COMMITTER_DATE", format!("{FIXED_DATE} +0800"))
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.args(args);
        cmd
    }

    fn git_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        self.git_cmd(args, envs).output().expect("spawn git")
    }

    fn git(&self, args: &[&str]) -> Output {
        self.git_env(args, &[])
    }

    fn git_ok(&self, args: &[&str]) -> Vec<u8> {
        let out = self.git(args);
        assert!(
            out.status.success(),
            "git {args:?} failed (exit {:?})\n--- stderr ---\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    fn git_text(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.git_ok(args))
            .trim_end()
            .to_string()
    }

    fn git_stdin(&self, args: &[&str], envs: &[(&str, &str)], stdin: &[u8]) -> Output {
        let mut cmd = self.git_cmd(args, envs);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn git");
        child
            .stdin
            .take()
            .expect("git stdin")
            .write_all(stdin)
            .expect("write git stdin");
        child.wait_with_output().expect("wait git")
    }

    fn init_git(&self) {
        let out = self.git(&["init", "-q", "-b", "main"]);
        assert!(
            out.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(self
            .git(&["config", "user.name", "Scenario Author"])
            .status
            .success());
        assert!(self
            .git(&["config", "user.email", "author@example.com"])
            .status
            .success());
    }

    // ----------------------------------------------------------------- mg

    fn mg(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(MG);
        cmd.current_dir(self.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", self.path())
            .env("LC_ALL", "C")
            .env("USER", "scenario")
            .env("HOSTNAME", "scenario-host")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        cmd.args(args);
        cmd.output().expect("spawn mg")
    }

    fn mg_ok(&self, args: &[&str]) -> Vec<u8> {
        let out = self.mg(args);
        assert!(
            out.status.success(),
            "mg {args:?} failed (exit {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
}

fn git_commit(sb: &Sandbox, message: &str, stamp: i64) {
    sb.git_ok(&["add", "-A"]);
    let date = format!("{stamp} +0800");
    let out = sb.git_env(
        &["commit", "-q", "-m", message],
        &[("GIT_AUTHOR_DATE", &date), ("GIT_COMMITTER_DATE", &date)],
    );
    assert!(
        out.status.success(),
        "git commit {message:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_merge(sb: &Sandbox, message: &str, stamp: i64, rev: &str) {
    let date = format!("{stamp} +0800");
    let out = sb.git_env(
        &["merge", "--no-ff", "-q", "-m", message, rev],
        &[("GIT_AUTHOR_DATE", &date), ("GIT_COMMITTER_DATE", &date)],
    );
    assert!(
        out.status.success(),
        "git merge {rev} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ===========================================================================
// 差分断言
// ===========================================================================

fn assert_bytes_eq(what: &str, got: &[u8], want: &[u8]) {
    if got == want {
        return;
    }
    panic!(
        "{what}: mg output differs from real git\n--- mg ({} bytes) ---\n{}\n--- git ({} bytes) ---\n{}\n{}",
        got.len(),
        String::from_utf8_lossy(got),
        want.len(),
        String::from_utf8_lossy(want),
        byte_diff(got, want)
    );
}

fn byte_diff(a: &[u8], b: &[u8]) -> String {
    let upto = a.len().min(b.len());
    for index in 0..upto {
        if a[index] != b[index] {
            return format!(
                "  first byte difference at offset {index}: mg={:#04x} git={:#04x}",
                a[index], b[index]
            );
        }
    }
    format!(
        "  common prefix equal; mg={} bytes, git={} bytes",
        a.len(),
        b.len()
    )
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// mg 的 `status --porcelain` 与真实 git 必须逐字节一致；同时用 `--porcelain=v1`、
/// `--short` 两个等价入口复验。
fn assert_status_matches_git(sb: &Sandbox, label: &str) -> Vec<u8> {
    let want = sb.git_ok(&["status", "--porcelain"]);
    let got = sb.mg_ok(&["status", "--porcelain"]);
    assert_bytes_eq(&format!("{label}: mg status --porcelain"), &got, &want);

    let want_v1 = sb.git_ok(&["status", "--porcelain=v1"]);
    assert_bytes_eq(
        &format!("{label}: mg status --porcelain=v1"),
        &sb.mg_ok(&["status", "--porcelain=v1"]),
        &want_v1,
    );
    let want_short = sb.git_ok(&["status", "--short"]);
    assert_bytes_eq(
        &format!("{label}: mg status --short"),
        &sb.mg_ok(&["status", "--short"]),
        &want_short,
    );

    // 库层：直接用 minigit::worktree::Worktree::status 渲染，必须与 CLI/git 三方一致。
    let repo = sb.open_repo();
    let index = Index::read(&repo).expect("read index");
    let head = head_tree_flat(&repo).expect("head tree");
    let direct = Worktree::new(&repo)
        .status(head.as_ref(), &index)
        .expect("library status")
        .porcelain();
    assert_bytes_eq(
        &format!("{label}: library Worktree::status"),
        direct.as_bytes(),
        &want,
    );
    got
}

/// 场景断言：真实 git 自己必须产出 `expected`（防止用例变成恒真），且 mg 与它逐字节一致。
fn assert_git_status_is(sb: &Sandbox, expected: &str, label: &str) {
    let want = sb.git_ok(&["status", "--porcelain"]);
    assert_eq!(
        String::from_utf8_lossy(&want),
        expected,
        "{label}: real git must produce the documented scenario bytes"
    );
    let got = sb.mg_ok(&["status", "--porcelain"]);
    assert_bytes_eq(&format!("{label}: mg status --porcelain"), &got, &want);
}

// ===========================================================================
// 库层：展平 HEAD tree（复制 cli::status 的只读语义，不调用 crate-private 函数）
// ===========================================================================

fn head_tree_flat(repo: &Repo) -> minigit::Result<Option<Tree>> {
    let store = RefStore::new(repo);
    let oid = match store.resolve("HEAD") {
        Ok(oid) => oid,
        Err(minigit::Error::RefNotFound(_)) => return Ok(None),
        Err(err) => return Err(err),
    };
    let odb = Odb::new(repo);
    let commit = odb.read_object(oid)?.into_commit()?;
    let mut entries = Vec::new();
    let mut prefix = Vec::new();
    flatten_tree(&odb, commit.tree, &mut prefix, &mut entries)?;
    Ok(Some(Tree::new(entries)))
}

fn flatten_tree(
    odb: &Odb<'_>,
    tree: Oid,
    prefix: &mut Vec<u8>,
    out: &mut Vec<TreeEntry>,
) -> minigit::Result<()> {
    let Object::Tree(subtree) = odb.read_object(tree)? else {
        return Err(minigit::Error::Other(format!("{tree} is not a tree")));
    };
    for entry in subtree.entries() {
        let base = prefix.len();
        if base > 0 {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(&entry.name);
        if entry.mode == FileMode::Tree {
            flatten_tree(odb, entry.oid, prefix, out)?;
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

// ===========================================================================
// 场景数据
// ===========================================================================

fn binary_blob(flavor: usize) -> Vec<u8> {
    let mut bytes = vec![0x00, 0x01, 0x02, 0x03, 0xff, 0xfe, 0x80];
    bytes.extend_from_slice(format!("\nblob-{flavor}\n").as_bytes());
    bytes.push(0x00);
    bytes
}

/// 只铺工作区文件（不提交），用于 `add` 对拍（含 ignore 与删除检测）。
fn populate_worktree(sb: &Sandbox, flavor: usize) {
    let dir = format!("dir{flavor}");
    sb.write_str("alpha.txt", "alpha\n");
    sb.write_bytes("bin.dat", &binary_blob(flavor));
    sb.write_str("\u{4e2d}\u{6587}.txt", "cjk\n");
    sb.write_str("sp ace.txt", "space\n");
    sb.write_str(&format!("{dir}/inner.txt"), "inner\n");
    sb.write_str(&format!("{dir}/deep/x.txt"), "x\n");
    sb.write_str(".gitignore", "*.log\nbuild/\n");
    sb.write_str("skip.log", "ignored\n");
    sb.write_str("build/out.txt", "build\n");
}

/// 用真实 git 建一个含分叉、合并、二进制、非 ASCII、空格路径的仓库。
/// `tie == true` 时让 side / m1 / m2 / merge 共用同一提交时间，专门考验 `log` 的同刻顺序。
fn populate_fork_merge(sb: &Sandbox, flavor: usize, tie: bool) {
    let dir = format!("dir{flavor}");
    sb.write_str("base.txt", &format!("base {flavor}\n"));
    sb.write_bytes("bin.dat", &binary_blob(flavor));
    sb.write_str("\u{4e2d}\u{6587}.txt", &format!("cjk {flavor}\n"));
    sb.write_str("sp ace.txt", "space\n");
    sb.write_str(&format!("{dir}/inner.txt"), "inner\n");
    sb.write_str(".gitignore", "*.log\nbuild/\n");
    sb.write_str("ignored.log", "ignored\n");
    sb.write_str("build/out.txt", "build\n");
    git_commit(sb, &format!("base {flavor}"), FIXED_DATE);

    let side_stamp = FIXED_DATE + 1;
    let m1_stamp = if tie { side_stamp } else { side_stamp + 1 };
    let m2_stamp = if tie { side_stamp } else { side_stamp + 2 };
    let merge_stamp = if tie { side_stamp } else { side_stamp + 3 };

    sb.git_ok(&["checkout", "-q", "-b", "side"]);
    sb.write_str("side.txt", &format!("side {flavor}\n"));
    git_commit(sb, "side", side_stamp);

    sb.git_ok(&["checkout", "-q", "main"]);
    sb.write_str("m1.txt", &format!("m1 {flavor}\n"));
    git_commit(sb, "m1", m1_stamp);
    sb.write_str("m2.txt", &format!("m2 {flavor}\n"));
    git_commit(sb, "m2", m2_stamp);

    git_merge(sb, &format!("merge side {flavor}"), merge_stamp, "side");
}

// ===========================================================================
// (B1) add
// ===========================================================================

fn parse_ls_files_stage(raw: &[u8]) -> Vec<(Vec<u8>, FileMode, Oid, u8)> {
    let mut out = Vec::new();
    for record in raw.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let tab = record.iter().position(|byte| *byte == b'\t').expect("tab");
        let header = std::str::from_utf8(&record[..tab]).expect("utf8 header");
        let path = record[tab + 1..].to_vec();
        let mut fields = header.split_whitespace();
        let mode = FileMode::from_bytes(fields.next().expect("mode").as_bytes()).expect("mode");
        let oid = Oid::from_hex(fields.next().expect("oid")).expect("oid");
        let stage: u8 = fields.next().expect("stage").parse().expect("stage");
        out.push((path, mode, oid, stage));
    }
    out.sort();
    out
}

#[test]
fn add_ls_files_stage_matches_git_across_three_repos() {
    for flavor in 0..3 {
        let git_sb = Sandbox::new();
        let mg_sb = Sandbox::new();
        for sb in [&git_sb, &mg_sb] {
            sb.init_git();
            populate_worktree(sb, flavor);
        }

        git_sb.git_ok(&["add", "."]);
        mg_sb.mg_ok(&["add", "."]);

        // 硬标准：真实 git 读 mg 写的 index，`ls-files --stage` 与它自己 `add .` 的结果全等。
        let want = git_sb.git_ok(&["ls-files", "--stage", "-z"]);
        let got = mg_sb.git_ok(&["ls-files", "--stage", "-z"]);
        assert_bytes_eq(
            &format!("add flavor {flavor}: ls-files --stage"),
            &got,
            &want,
        );

        // 库层交叉校验：minigit::index::Index 的 path/mode/oid 与 git 完全一致。
        let repo = mg_sb.open_repo();
        let index = Index::read(&repo).expect("read index");
        let parsed = parse_ls_files_stage(&want);
        assert_eq!(
            index.entries.len(),
            parsed.len(),
            "entry count flavor {flavor}"
        );
        for ((path, mode, oid, stage), entry) in parsed.iter().zip(index.entries.iter()) {
            assert_eq!(&entry.path, path, "path flavor {flavor}");
            assert_eq!(entry.mode, *mode, "mode for {path:?}");
            assert_eq!(entry.oid, *oid, "oid for {path:?}");
            assert_eq!(entry.stage, *stage, "stage for {path:?}");
        }

        // 删除检测：工作区删掉一个文件 + 整个目录后，`add <dir>` 必须把条目从 index 移除。
        std::fs::remove_file(git_sb.join(&format!("dir{flavor}/inner.txt"))).unwrap();
        std::fs::remove_file(mg_sb.join(&format!("dir{flavor}/inner.txt"))).unwrap();
        std::fs::remove_dir_all(git_sb.join(&format!("dir{flavor}/deep"))).unwrap();
        std::fs::remove_dir_all(mg_sb.join(&format!("dir{flavor}/deep"))).unwrap();
        git_sb.git_ok(&["add", "."]);
        mg_sb.mg_ok(&["add", "."]);

        let want = git_sb.git_ok(&["ls-files", "--stage", "-z"]);
        let got = mg_sb.git_ok(&["ls-files", "--stage", "-z"]);
        assert_bytes_eq(
            &format!("add flavor {flavor}: deletion detection"),
            &got,
            &want,
        );
    }
}

#[test]
fn add_nonexistent_path_exits_nonzero_with_stderr_and_no_panic() {
    let sb = Sandbox::new();
    sb.init_git();
    sb.write_str("a.txt", "a\n");

    let out = sb.mg(&["add", "missing/path.txt"]);
    assert!(!out.status.success(), "add of a missing path must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.trim().is_empty(), "must explain itself on stderr");
    assert!(!stderr.contains("panicked"), "must not panic: {stderr}");
    // 失败不得改动 index。
    assert!(sb.git_text(&["ls-files"]).is_empty());
}

#[test]
fn add_refuses_an_explicitly_ignored_untracked_path() {
    let sb = Sandbox::new();
    sb.init_git();
    sb.write_str(".gitignore", "*.log\n");
    sb.write_str("keep.txt", "keep\n");
    sb.write_str("drop.log", "drop\n");

    // 目录递归命中被 ignore 的文件只是跳过；显式点名的被 ignore 路径必须拒绝。
    sb.mg_ok(&["add", "."]);
    assert!(!sb.git_text(&["ls-files"]).contains("drop.log"));
    assert!(sb.git_text(&["ls-files"]).contains("keep.txt"));

    let out = sb.mg(&["add", "drop.log"]);
    assert!(
        !out.status.success(),
        "explicit ignored path must be refused"
    );
}

// ===========================================================================
// (B1) rm
// ===========================================================================

#[test]
fn rm_and_rm_cached_match_git_status() {
    for flavor in 0..3 {
        let git_sb = Sandbox::new();
        let mg_sb = Sandbox::new();
        for sb in [&git_sb, &mg_sb] {
            sb.init_git();
            populate_fork_merge(sb, flavor, flavor == 2);
        }

        // 默认 `rm`：删 index + 删工作区文件。
        git_sb.git_ok(&["rm", "-q", "base.txt"]);
        mg_sb.mg_ok(&["rm", "base.txt"]);
        assert_status_matches_git(&git_sb, &format!("rm flavor {flavor}: git side"));
        assert_status_matches_git(&mg_sb, &format!("rm flavor {flavor}: mg side"));
        assert_eq!(
            git_sb.git_ok(&["status", "--porcelain"]),
            mg_sb.git_ok(&["status", "--porcelain"]),
            "rm flavor {flavor}: porcelain must match after rm"
        );
        assert!(!git_sb.exists("base.txt") && !mg_sb.exists("base.txt"));

        // `--cached`：只删 index，工作区与 HEAD 不动。
        git_sb.git_ok(&["rm", "-q", "--cached", "\u{4e2d}\u{6587}.txt"]);
        mg_sb.mg_ok(&["rm", "--cached", "\u{4e2d}\u{6587}.txt"]);
        assert_eq!(
            git_sb.git_ok(&["status", "--porcelain"]),
            mg_sb.git_ok(&["status", "--porcelain"]),
            "rm --cached flavor {flavor}: porcelain must match"
        );
        assert!(git_sb.exists("\u{4e2d}\u{6587}.txt") && mg_sb.exists("\u{4e2d}\u{6587}.txt"));
    }
}

#[test]
fn rm_without_force_refuses_and_leaves_zero_changes() {
    let sb = Sandbox::new();
    sb.init_git();
    populate_fork_merge(&sb, 0, false);

    sb.write_str("base.txt", "locally modified\n");
    let work_before = sb.read("base.txt");
    let index_before = sb.read(".git/index");

    let out = sb.mg(&["rm", "base.txt"]);
    assert!(
        !out.status.success(),
        "rm must refuse a locally modified file"
    );
    assert!(
        !out.stderr.is_empty(),
        "refusal must be explained on stderr"
    );

    // 零改动：工作区字节、index 字节都必须原样。
    assert_eq!(
        sb.read("base.txt"),
        work_before,
        "worktree must be untouched"
    );
    assert_eq!(
        sb.read(".git/index"),
        index_before,
        "index must be untouched"
    );

    // 真值：真实 git 也拒绝同样的场景。
    let git_out = sb.git(&["rm", "base.txt"]);
    assert!(!git_out.status.success(), "real git must also refuse");
}

// ===========================================================================
// (B1/B2) status，含 T
// ===========================================================================

#[test]
fn status_porcelain_matches_git_byte_for_byte() {
    let sb = Sandbox::new();
    sb.init_git();
    populate_fork_merge(&sb, 0, false);

    // 已跟踪、未暂存的改动与删除。
    sb.write_str("base.txt", "changed\n");
    sb.remove("m1.txt");
    // 已暂存的改动。
    sb.write_str("side.txt", "staged change\n");
    sb.git_ok(&["add", "side.txt"]);
    // 未跟踪：一个排在已跟踪变更之前的名字 + 目录折叠 + 空格 + 非 ASCII + ignored。
    sb.write_str("aaa.txt", "untracked\n");
    sb.write_str("untracked-dir/a.txt", "a\n");
    sb.write_str("untracked-dir/sub/b.txt", "b\n");
    sb.write_str("new dir/n.txt", "n\n");
    sb.write_str("\u{65b0}\u{589e}.txt", "n\n");
    sb.write_str("more.log", "ignored\n");
    // 工作区类型变化（普通文件 → symlink）。
    sb.remove("sp ace.txt");
    std::os::unix::fs::symlink("base.txt", sb.join("sp ace.txt")).expect("symlink");

    let want = assert_status_matches_git(&sb, "rich status");

    // 分组顺序必须与 git 一致：先已跟踪变更，再 `??`；不能是全局字节序。
    let tracked = find_sub(&want, b" M base.txt\n").expect("tracked change present");
    let untracked = find_sub(&want, b"?? aaa.txt\n").expect("untracked present");
    assert!(
        tracked < untracked,
        "git groups tracked changes before untracked entries (truth source self-check)"
    );
    // 引号转义：含空格的路径必须按 C 风格加引号（与 git 一致，上面逐字节比较已覆盖）。
    assert!(
        find_sub(&want, b"\"sp ace.txt\"").is_some(),
        "space path must be quoted like git: {}",
        String::from_utf8_lossy(&want)
    );
}

#[test]
fn status_porcelain_in_unborn_repository_matches_git() {
    let sb = Sandbox::new();
    sb.init_git();
    sb.write_str("staged.txt", "s\n");
    sb.git_ok(&["add", "staged.txt"]);
    sb.write_str("untracked.txt", "u\n");
    assert_status_matches_git(&sb, "unborn repo");
}

#[cfg(unix)]
#[test]
fn type_change_status_matches_git_byte_for_byte() {
    // 场景 A：index 里是普通文件，工作区换成 symlink → ` T`。
    let a = Sandbox::new();
    a.init_git();
    a.write_str("f", "plain\n");
    a.write_str("target", "target\n");
    git_commit(&a, "init", FIXED_DATE);
    a.remove("f");
    std::os::unix::fs::symlink("target", a.join("f")).expect("symlink");
    assert_git_status_is(&a, " T f\n", "regular -> symlink (worktree)");

    // 场景 B：symlink → 普通文件，即使内容撞巧等于原 symlink 的 blob 也仍是 ` T`。
    let b = Sandbox::new();
    b.init_git();
    b.write_str("target", "target\n");
    std::os::unix::fs::symlink("target", b.join("link")).expect("symlink");
    git_commit(&b, "init", FIXED_DATE);
    b.remove("link");
    b.write_str("link", "target"); // 与 symlink blob 逐字节相同
    assert_git_status_is(&b, " T link\n", "symlink -> regular (identical bytes)");

    // 场景 C：HEAD 普通文件、index symlink → `T `（staged 类型变化）。
    let c = Sandbox::new();
    c.init_git();
    c.write_str("f", "plain\n");
    c.write_str("other", "other\n");
    git_commit(&c, "init", FIXED_DATE);
    c.remove("f");
    std::os::unix::fs::symlink("other", c.join("f")).expect("symlink");
    c.git_ok(&["add", "f"]);
    assert_git_status_is(&c, "T  f\n", "staged regular -> symlink");

    // 场景 D：`MT` —— index 相对 HEAD 已改（内容），工作区又类型变化。
    let d = Sandbox::new();
    d.init_git();
    d.write_str("f", "plain\n");
    d.write_str("other", "other\n");
    git_commit(&d, "init", FIXED_DATE);
    d.write_str("f", "changed\n");
    d.git_ok(&["add", "f"]);
    d.remove("f");
    std::os::unix::fs::symlink("other", d.join("f")).expect("symlink");
    assert_git_status_is(&d, "MT f\n", "staged mod + worktree type change");
}

// ===========================================================================
// (B1) commit
// ===========================================================================

fn split_commit(raw: &[u8]) -> (Vec<String>, Vec<u8>) {
    let pos = raw
        .windows(2)
        .position(|window| window == b"\n\n")
        .expect("commit has a blank line");
    let headers = String::from_utf8_lossy(&raw[..pos])
        .lines()
        .map(str::to_string)
        .collect();
    (headers, raw[pos + 2..].to_vec())
}

/// `author Name <email> 1700000000 +0800` → (name, email, "1700000000 +0800")。
fn parse_signature(line: &str) -> (String, String, String) {
    let open = line.find('<').expect("signature '<'");
    let close = line.find('>').expect("signature '>'");
    let head = line.split_once(' ').expect("signature type").1;
    let name = head[..head.find('<').expect("name end")].trim().to_string();
    let email = line[open + 1..close].to_string();
    let date = line[close + 2..].to_string();
    (name, email, date)
}

/// 用真实 `git commit-tree` 按 mg 提交里的 tree/parent/author/committer/消息重放一遍，
/// 返回重放出的 commit oid。两者对象载荷必须逐字节相同 —— 这是「编码与 git 一致」的硬证据。
fn replay_commit_with_git(sb: &Sandbox, raw: &[u8]) -> String {
    let (headers, body) = split_commit(raw);
    let tree = headers
        .iter()
        .find_map(|line| line.strip_prefix("tree "))
        .expect("tree header")
        .to_string();
    let parents: Vec<String> = headers
        .iter()
        .filter_map(|line| line.strip_prefix("parent ").map(str::to_string))
        .collect();
    let author = headers
        .iter()
        .find(|line| line.starts_with("author "))
        .expect("author header");
    let committer = headers
        .iter()
        .find(|line| line.starts_with("committer "))
        .expect("committer header");
    let (author_name, author_email, author_date) = parse_signature(author);
    let (committer_name, committer_email, committer_date) = parse_signature(committer);

    let mut args: Vec<&str> = vec!["commit-tree", &tree];
    for parent in &parents {
        args.push("-p");
        args.push(parent);
    }
    let envs = [
        ("GIT_AUTHOR_NAME", author_name.as_str()),
        ("GIT_AUTHOR_EMAIL", author_email.as_str()),
        ("GIT_COMMITTER_NAME", committer_name.as_str()),
        ("GIT_COMMITTER_EMAIL", committer_email.as_str()),
        ("GIT_AUTHOR_DATE", author_date.as_str()),
        ("GIT_COMMITTER_DATE", committer_date.as_str()),
    ];
    let out = sb.git_stdin(&args, &envs, &body);
    assert!(
        out.status.success(),
        "git commit-tree replay failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn assert_fsck_clean(sb: &Sandbox, label: &str) {
    let out = sb.git(&["fsck", "--no-progress"]);
    assert!(out.status.success(), "{label}: git fsck must exit 0");
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        !stderr.contains("error") && !stderr.contains("fatal"),
        "{label}: git fsck reported an error:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn system_tz() -> Option<String> {
    let out = Command::new("date").arg("+%z").output().ok()?;
    let tz = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (tz.len() == 5).then_some(tz)
}

#[test]
fn commit_encoding_tree_parents_and_identity_match_git() {
    for flavor in 0..3 {
        let sb = Sandbox::new();
        sb.init_git();
        populate_fork_merge(&sb, flavor, flavor == 2);

        // 用 mg 暂存一处改动（含新增子目录文件）。
        sb.write_str("base.txt", &format!("changed {flavor}\n"));
        sb.write_str(&format!("dir{flavor}/new.txt"), "new\n");
        sb.mg_ok(&["add", "."]);

        // 真值 1：git 自己对同一 index 算出的 tree。
        let expected_tree = sb.git_text(&["write-tree"]);
        let old_head = sb.git_text(&["rev-parse", "HEAD"]);

        let out = sb.mg(&["commit", "-m", &format!("plumbing commit {flavor}")]);
        assert!(
            out.status.success(),
            "mg commit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        assert_fsck_clean(&sb, &format!("commit flavor {flavor}"));

        // 真值 2：tree 与 parent 必须等于 git 的独立计算。
        assert_eq!(
            sb.git_text(&["log", "-1", "--format=%T"]),
            expected_tree,
            "commit tree flavor {flavor}"
        );
        assert_eq!(
            sb.git_text(&["log", "-1", "--format=%P"]),
            old_head,
            "commit parent flavor {flavor}"
        );

        // 库层：minigit::odb / refs 读到的 oid/tree/parent 与 git 一致。
        let repo = sb.open_repo();
        let store = RefStore::new(&repo);
        let head = store.resolve("HEAD").expect("resolve HEAD");
        assert_eq!(head.to_hex(), sb.git_text(&["rev-parse", "HEAD"]));
        let odb = Odb::new(&repo);
        let commit = match odb.read_object(head).expect("read commit") {
            Object::Commit(commit) => commit,
            other => panic!("HEAD is not a commit: {}", other.kind()),
        };
        assert_eq!(commit.tree.to_hex(), expected_tree);
        assert_eq!(commit.parents.len(), 1, "one parent for a normal commit");
        assert_eq!(commit.parents[0].to_hex(), old_head);
        assert_eq!(
            String::from_utf8_lossy(&commit.author.name),
            "Scenario Author"
        );
        assert_eq!(
            String::from_utf8_lossy(&commit.author.email),
            "author@example.com"
        );
        assert_eq!(
            String::from_utf8_lossy(&commit.committer.name),
            "Scenario Author"
        );
        assert_eq!(
            String::from_utf8_lossy(&commit.committer.email),
            "author@example.com"
        );
        // 时区格式必须是 `<epoch> ±HHMM`，且等于系统的真实偏移（`date +%z`）。
        assert_eq!(commit.committer.tz.len(), 5, "tz must be ±HHMM");
        assert!(commit.committer.tz.starts_with('+') || commit.committer.tz.starts_with('-'));
        if let Some(system) = system_tz() {
            assert_eq!(
                commit.committer.tz, system,
                "commit flavor {flavor}: mg tz must match `date +%z`"
            );
            assert_eq!(commit.author.tz, system, "author tz flavor {flavor}");
        }

        // 真值 2b：`git log --format=raw` 的 tree/parent/author/committer 行逐字一致。
        let raw_log =
            String::from_utf8_lossy(&sb.git_ok(&["log", "--format=raw", "-1"])).into_owned();
        assert!(
            raw_log.contains(&format!("tree {expected_tree}\n")),
            "{raw_log}"
        );
        assert!(
            raw_log.contains(&format!("parent {old_head}\n")),
            "{raw_log}"
        );
        assert!(
            raw_log.contains("author Scenario Author <author@example.com> "),
            "{raw_log}"
        );
        assert!(
            raw_log.contains("committer Scenario Author <author@example.com> "),
            "{raw_log}"
        );

        // 真值 3：用 git commit-tree 重放，载荷必须逐字节相同。
        let raw = sb.git_ok(&["cat-file", "commit", "HEAD"]);
        let replayed = replay_commit_with_git(&sb, &raw);
        let replayed_raw = sb.git_ok(&["cat-file", "commit", &replayed]);
        assert_bytes_eq(
            &format!("commit flavor {flavor}: payload vs git commit-tree"),
            &raw,
            &replayed_raw,
        );
    }
}

#[test]
fn commit_author_override_sets_author_only() {
    let sb = Sandbox::new();
    sb.init_git();
    sb.write_str("a.txt", "a\n");
    git_commit(&sb, "base", FIXED_DATE);
    sb.write_str("a.txt", "b\n");
    sb.mg_ok(&["add", "a.txt"]);

    let out = sb.mg(&[
        "commit",
        "-m",
        "authored",
        "--author",
        "Override Name <override@example.com>",
    ]);
    assert!(
        out.status.success(),
        "mg commit --author failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(
        sb.git_text(&["log", "-1", "--format=%an <%ae>"]),
        "Override Name <override@example.com>"
    );
    assert_eq!(
        sb.git_text(&["log", "-1", "--format=%cn <%ce>"]),
        "Scenario Author <author@example.com>",
        "committer must not be overridden"
    );
    assert_fsck_clean(&sb, "author override");
}

#[test]
fn commit_amend_allow_empty_and_no_change_refusal_match_git_semantics() {
    let sb = Sandbox::new();
    sb.init_git();
    populate_fork_merge(&sb, 0, false);

    // 无改动、无 --allow-empty → 拒绝（且不得产生新提交）。
    let head_before = sb.git_text(&["rev-parse", "HEAD"]);
    let out = sb.mg(&["commit", "-m", "nothing to commit"]);
    assert!(!out.status.success(), "clean tree commit must be refused");
    assert_eq!(sb.git_text(&["rev-parse", "HEAD"]), head_before);

    // --allow-empty → 成功，且 tree 与 parent 相同。
    let out = sb.mg(&["commit", "--allow-empty", "-m", "empty"]);
    assert!(
        out.status.success(),
        "allow-empty failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let empty_head = sb.git_text(&["rev-parse", "HEAD"]);
    assert_ne!(empty_head, head_before);
    let trees = sb.git_text(&["log", "-2", "--format=%T"]);
    let trees: Vec<&str> = trees.lines().collect();
    assert_eq!(trees[0], trees[1], "empty commit keeps the same tree");

    // --amend → 替换 HEAD，parent 链沿用被 amend 提交的 parent（此处即原 HEAD）。
    let parents_before = sb.git_text(&["log", "-1", "--format=%P"]);
    let out = sb.mg(&["commit", "--amend", "-m", "amended"]);
    assert!(
        out.status.success(),
        "amend failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_ne!(sb.git_text(&["rev-parse", "HEAD"]), empty_head);
    assert_eq!(sb.git_text(&["log", "-1", "--format=%P"]), parents_before);
    assert_eq!(sb.git_text(&["log", "-1", "--format=%s"]), "amended");
    assert_fsck_clean(&sb, "amend");
}

#[test]
fn commit_all_stages_tracked_changes() {
    let sb = Sandbox::new();
    sb.init_git();
    populate_fork_merge(&sb, 0, false);

    sb.write_str("base.txt", "all change\n");
    sb.remove("side.txt");
    sb.write_str("untracked.txt", "u\n");

    let out = sb.mg(&["commit", "-a", "-m", "all"]);
    assert!(
        out.status.success(),
        "commit -a failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(sb.git_text(&["show", "HEAD:base.txt"]), "all change");
    let names = sb.git_text(&["ls-tree", "-r", "--name-only", "HEAD"]);
    assert!(!names.contains("side.txt"), "deletion must be staged by -a");
    assert!(
        !names.contains("untracked.txt"),
        "-a must not add untracked files"
    );
    assert!(
        sb.exists("untracked.txt"),
        "-a must not delete untracked files"
    );
    assert_fsck_clean(&sb, "commit -a");
}

#[test]
fn commit_refuses_during_conflict_and_finishes_merge_after_resolution() {
    let sb = Sandbox::new();
    sb.init_git();
    sb.write_str("f.txt", "base\n");
    sb.write_str("anchor.txt", "anchor\n");
    git_commit(&sb, "base", FIXED_DATE);
    sb.git_ok(&["checkout", "-q", "-b", "side"]);
    sb.write_str("f.txt", "side\n");
    git_commit(&sb, "side", FIXED_DATE + 1);
    sb.git_ok(&["checkout", "-q", "main"]);
    sb.write_str("f.txt", "main\n");
    git_commit(&sb, "main", FIXED_DATE + 2);

    let merge = sb.git(&["merge", "side"]);
    assert!(!merge.status.success(), "scenario must produce a conflict");
    assert!(sb.exists(".git/MERGE_HEAD"));

    // 冲突未解决时 mg commit 必须拒绝。
    let out = sb.mg(&["commit", "-m", "must refuse"]);
    assert!(!out.status.success(), "commit must refuse unmerged index");
    assert!(
        sb.git_text(&["status", "--porcelain"]).contains("UU f.txt"),
        "conflict is still there"
    );

    // 解决后用 mg 收尾 merge：两个 parent，且清理 MERGE_HEAD/MERGE_MSG。
    let first_parent = sb.git_text(&["rev-parse", "HEAD"]);
    let second_parent = sb.git_text(&["rev-parse", "side"]);
    sb.write_str("f.txt", "resolved\n");
    sb.git_ok(&["add", "f.txt"]);
    let out = sb.mg(&["commit", "-m", "merge side"]);
    assert!(
        out.status.success(),
        "mg must finish the merge: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        sb.git_text(&["log", "-1", "--format=%P"]),
        format!("{first_parent} {second_parent}"),
        "merge commit must have exactly two parents in order"
    );
    assert!(!sb.exists(".git/MERGE_HEAD"));
    assert!(!sb.exists(".git/MERGE_MSG"));
    assert_fsck_clean(&sb, "merge finish");
}

// ===========================================================================
// (B1) log
// ===========================================================================

#[test]
fn log_oneline_matches_git_across_three_repos() {
    for flavor in 0..3 {
        let sb = Sandbox::new();
        sb.init_git();
        populate_fork_merge(&sb, flavor, flavor == 2);
        let label = format!("log flavor {flavor}");

        let want = sb.git_ok(&["log", "--oneline"]);
        let got = sb.mg_ok(&["log", "--oneline"]);
        assert_bytes_eq(&format!("{label}: default"), &got, &want);

        let want_n = sb.git_ok(&["log", "--oneline", "-n", "3"]);
        let got_n = sb.mg_ok(&["log", "--oneline", "-n", "3"]);
        assert_bytes_eq(&format!("{label}: -n 3"), &got_n, &want_n);

        let want_side = sb.git_ok(&["log", "--oneline", "side"]);
        let got_side = sb.mg_ok(&["log", "--oneline", "side"]);
        assert_bytes_eq(&format!("{label}: rev=side"), &got_side, &want_side);
    }
}

// ===========================================================================
// (B1) tag
// ===========================================================================

/// 把 tag 载荷里的 `tag <name>` 行抹平成固定占位（两个 tag 的名字不同，其余必须一致）。
fn normalize_tag_payload(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    for line in raw.split_inclusive(|byte| *byte == b'\n') {
        if line.starts_with(b"tag ") {
            out.extend_from_slice(b"tag <normalized>\n");
        } else {
            out.extend_from_slice(line);
        }
    }
    out
}

#[test]
fn tag_objects_listing_and_types_match_git() {
    let sb = Sandbox::new();
    sb.init_git();
    populate_fork_merge(&sb, 0, false);

    sb.mg_ok(&["tag", "v1.0"]); // 轻量
    sb.mg_ok(&["tag", "-a", "-m", "annotated message", "v2.0"]);
    sb.mg_ok(&["tag", "-m", "implicit annotated", "v3.0"]); // -m 隐含 -a
    sb.mg_ok(&["tag", "-a", "-m", "at rev", "v4.0", "side"]);

    // 列表输出与 git 逐字节一致（含通配过滤）。
    assert_bytes_eq(
        "tag -l",
        &sb.mg_ok(&["tag", "-l"]),
        &sb.git_ok(&["tag", "-l"]),
    );
    assert_bytes_eq(
        "tag -l v1*",
        &sb.mg_ok(&["tag", "-l", "v1*"]),
        &sb.git_ok(&["tag", "-l", "v1*"]),
    );

    // objecttype 真值由 git 给出。
    let types = sb.git_text(&[
        "for-each-ref",
        "--format=%(refname:short) %(objecttype)",
        "refs/tags",
    ]);
    assert!(
        types.contains("v1.0 commit"),
        "lightweight points at a commit: {types}"
    );
    assert!(
        types.contains("v2.0 tag"),
        "annotated is a tag object: {types}"
    );
    assert!(types.contains("v3.0 tag"), "-m implies -a: {types}");
    assert!(types.contains("v4.0 tag"), "{types}");

    // annotated tag 的载荷必须与「同 object/同消息/同 tagger 时间」的 git tag 逐字节一致。
    let mg_payload = sb.git_ok(&["cat-file", "tag", "refs/tags/v2.0"]);
    let tagger_date = sb.git_text(&[
        "for-each-ref",
        "--format=%(taggerdate:raw)",
        "refs/tags/v2.0",
    ]);
    let out = sb.git_env(
        &["tag", "-a", "-m", "annotated message", "git-v2"],
        &[("GIT_COMMITTER_DATE", &tagger_date)],
    );
    assert!(
        out.status.success(),
        "git reference tag failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let git_payload = sb.git_ok(&["cat-file", "tag", "refs/tags/git-v2"]);
    assert_bytes_eq(
        "tag payload vs git tag",
        &normalize_tag_payload(&mg_payload),
        &normalize_tag_payload(&git_payload),
    );

    // 已存在的 tag 不得被覆盖。
    let out = sb.mg(&["tag", "v1.0"]);
    assert!(!out.status.success(), "duplicate tag must be refused");
    assert_fsck_clean(&sb, "tags");
}

// ===========================================================================
// (B3) 双向互操作
// ===========================================================================

#[test]
fn interop_mg_written_repo_is_usable_by_real_git() {
    let sb = Sandbox::new();
    let out = sb.mg(&["init", "."]);
    assert!(
        out.status.success(),
        "mg init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    sb.git_ok(&["config", "user.name", "Scenario Author"]);
    sb.git_ok(&["config", "user.email", "author@example.com"]);

    sb.write_str("a.txt", "a\n");
    sb.write_bytes("bin.dat", &binary_blob(7));
    sb.write_str("\u{4e2d}\u{6587}.txt", "cjk\n");
    sb.mg_ok(&["add", "."]);

    // 方向 1a：真实 git 能直接在 mg 写的 index 上提交（state 兼容）。
    let date = format!("{FIXED_DATE} +0800");
    let out = sb.git_env(
        &["commit", "-q", "-m", "git commits on mg index"],
        &[("GIT_AUTHOR_DATE", &date), ("GIT_COMMITTER_DATE", &date)],
    );
    assert!(
        out.status.success(),
        "real git must commit on mg's index: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 方向 1b：mg 继续提交 + 打 tag，再让真实 git 读取。
    sb.write_str("b.txt", "b\n");
    sb.mg_ok(&["add", "."]);
    let out = sb.mg(&["commit", "-m", "mg commit"]);
    assert!(
        out.status.success(),
        "mg commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    sb.mg_ok(&["tag", "v1"]);

    assert!(sb.git(&["status", "--porcelain"]).status.success());
    assert_eq!(sb.git_text(&["log", "--oneline"]).lines().count(), 2);
    assert_eq!(sb.git_text(&["tag", "-l"]), "v1");
    assert_eq!(sb.git_text(&["log", "-1", "--format=%s"]), "mg commit");
    assert_fsck_clean(&sb, "mg-written repo");

    // 方向 1c：真实 git 在 mg 的提交之上继续开发。
    sb.write_str("c.txt", "c\n");
    sb.git_ok(&["add", "."]);
    let out = sb.git_env(
        &["commit", "-q", "-m", "git on top of mg"],
        &[("GIT_AUTHOR_DATE", &date), ("GIT_COMMITTER_DATE", &date)],
    );
    assert!(out.status.success());
    assert_eq!(sb.git_text(&["log", "--oneline"]).lines().count(), 3);
    assert_fsck_clean(&sb, "git on top of mg");
}

#[test]
fn interop_git_written_repo_is_read_correctly_by_mg() {
    for flavor in 0..3 {
        let sb = Sandbox::new();
        sb.init_git();
        populate_fork_merge(&sb, flavor, flavor == 2);
        let label = format!("git->mg flavor {flavor}");

        // 读：status 与 log 都必须与真实 git 一致。
        assert_bytes_eq(
            &format!("{label}: status"),
            &sb.mg_ok(&["status", "--porcelain"]),
            &sb.git_ok(&["status", "--porcelain"]),
        );
        assert_bytes_eq(
            &format!("{label}: log"),
            &sb.mg_ok(&["log", "--oneline"]),
            &sb.git_ok(&["log", "--oneline"]),
        );

        // 库层：HEAD 解析、commit tree 与 git 一致。
        let repo = sb.open_repo();
        let store = RefStore::new(&repo);
        assert_eq!(
            store.resolve("HEAD").expect("resolve").to_hex(),
            sb.git_text(&["rev-parse", "HEAD"])
        );

        // 写：mg 在 git 写的仓库上继续 add + commit，真实 git 必须读得懂。
        sb.write_str("on-top.txt", "on top\n");
        sb.mg_ok(&["add", "."]);
        let out = sb.mg(&["commit", "-m", "on top"]);
        assert!(
            out.status.success(),
            "{label}: mg commit on git repo failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(sb.git_text(&["log", "-1", "--format=%s"]), "on top");
        assert_fsck_clean(&sb, &label);
    }
}

// ===========================================================================
// 已知限制 / 未覆盖（不是失败，用例只做「按契约响亮失败」的确认）
// ===========================================================================

#[test]
fn known_limitations_fail_loudly_instead_of_pretending() {
    let sb = Sandbox::new();
    sb.init_git();
    sb.write_str("a.txt", "a\n");
    git_commit(&sb, "base", FIXED_DATE);

    // --porcelain=v2 明确 NotImplemented（任务书允许的已知限制）。
    let out = sb.mg(&["status", "--porcelain=v2"]);
    assert!(!out.status.success(), "v2 must not pretend to succeed");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not implemented"),
        "v2 must explain NotImplemented: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 无参数目录的 rm 需要 -r；签名里没有 -r，必须响亮拒绝而不是偷偷递归。
    sb.write_str("dir/x.txt", "x\n");
    sb.mg_ok(&["add", "dir"]);
    let out = sb.mg(&["rm", "dir"]);
    assert!(
        !out.status.success(),
        "rm of a directory without -r must refuse"
    );
}

// ===========================================================================
// 额外：mg log 在 unborn HEAD 上必须响亮失败而不是假装成功
// ===========================================================================

#[test]
fn log_in_unborn_repository_fails_loudly() {
    let sb = Sandbox::new();
    sb.init_git();
    let out = sb.mg(&["log", "--oneline"]);
    assert!(!out.status.success());
    assert!(!out.stderr.is_empty());
}
