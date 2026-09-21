//! V4 独立验证（T4：worktree scan / ignore / status）。
//!
//! 纪律（本文件的核心约束）：
//!
//! * **期望值零硬编码**：所有真值都由运行时的真实 `git` 二进制现场产生
//!   （`git status --porcelain` / `git check-ignore` / `git ls-files`）。
//! * **输入也来自 git**：`head_tree` 由 `git ls-tree -r -z HEAD` 解析成**展平** tree
//!   （entry 的 name 是仓库相对全路径），`Index` 由 `git ls-files --stage -z` 解析
//!   （含 stage 字段与 6 位八进制 mode）。
//! * 被测对象只有 `minigit::worktree` 的公开 API；本文件里**没有**任何自造的
//!   status / ignore 判定逻辑（`walk_worktree` 只是枚举文件路径，用作喂给
//!   `git check-ignore` 的输入清单，不参与任何断言的真值计算）。
//! * 每个场景使用独立 `tempfile::tempdir()`，互不干扰。
//!
//! 少数 `assert_contains` 里的短字符串是「场景确实造出了预期状态」的自检
//! （断言的是 **git 的输出**，不是被测函数的输出），用于防止假场景。
#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink as unix_symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use minigit::index::{Index, IndexEntry};
use minigit::object::{FileMode, Tree, TreeEntry};
use minigit::oid::Oid;
use minigit::repo::Repo;
use minigit::worktree::{Ignore, Worktree};

// ---------------------------------------------------------------- scratch repo

/// 一个由真实 `git` 建立、隔离在 tempdir 里的仓库。所有真值都从它现场取。
struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Scratch {
        let dir = tempfile::tempdir().expect("tempdir");
        let scratch = Scratch { dir };
        scratch.git_ok(&["init", "-q", "-b", "main"]);
        scratch
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn join(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn write(&self, rel: &str, contents: &str) {
        let path = self.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write file");
    }

    fn write_bytes(&self, rel: &[u8], contents: &[u8]) {
        let path = self.path().join(std::ffi::OsStr::from_bytes(rel));
        fs::write(path, contents).expect("write bytes");
    }

    fn mkdir(&self, rel: &str) {
        fs::create_dir_all(self.join(rel)).expect("mkdir");
    }

    fn remove(&self, rel: &str) {
        fs::remove_file(self.join(rel)).expect("remove file");
    }

    fn chmod_exec(&self, rel: &str) {
        let path = self.join(rel);
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    fn symlink(&self, target: &str, rel: &str) {
        unix_symlink(target, self.join(rel)).expect("symlink");
    }

    /// 干净环境里的 git：不受宿主 `~/.gitconfig`、`GIT_*` 变量影响。
    fn git(&self, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(self.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.path())
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("GIT_TERMINAL_PROMPT", "0")
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

    fn commit(&self, message: &str) {
        self.git_ok(&["add", "-A"]);
        self.git_ok(&["commit", "-q", "-m", message]);
    }

    /// 真值：真实 git 的 `status --porcelain` 原始 stdout（含末尾换行）。
    fn truth(&self) -> String {
        self.git_text(&["status", "--porcelain"])
    }

    /// `git ls-tree -r -z HEAD` → **展平**的 `Tree`（name 是仓库相对全路径）。
    /// HEAD 不存在（无提交）时返回 `None`。
    fn head_tree(&self) -> Option<Tree> {
        if !self
            .git(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .status
            .success()
        {
            return None;
        }
        let raw = self.git_ok(&["ls-tree", "-r", "-z", "HEAD"]);
        let mut entries = Vec::new();
        for record in split_nul(&raw) {
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .expect("ls-tree record has a tab");
            let header = std::str::from_utf8(&record[..tab]).expect("ls-tree header utf8");
            let name = record[tab + 1..].to_vec();
            let mut fields = header.split_whitespace();
            let mode = fields.next().expect("mode");
            let _kind = fields.next().expect("type");
            let oid = fields.next().expect("oid");
            // `160000`（gitlink）等不在 v1 范围；`FileMode` 认不出就跳过。
            if let Ok(mode) = FileMode::from_bytes(mode.as_bytes()) {
                entries.push(TreeEntry {
                    mode,
                    name,
                    oid: Oid::from_hex(oid).expect("oid hex"),
                });
            }
        }
        Some(Tree::new(entries))
    }

    /// `git ls-files --stage -z` → `Index`（含 stage 1/2/3）。
    fn index(&self) -> Index {
        let raw = self.git_ok(&["ls-files", "--stage", "-z"]);
        let mut entries = Vec::new();
        for record in split_nul(&raw) {
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .expect("ls-files record has a tab");
            let header = std::str::from_utf8(&record[..tab]).expect("ls-files header utf8");
            let path = record[tab + 1..].to_vec();
            let mut fields = header.split_whitespace();
            let mode = FileMode::from_bytes(fields.next().expect("mode").as_bytes())
                .expect("6-digit octal mode");
            let oid = Oid::from_hex(fields.next().expect("oid")).expect("oid hex");
            let stage: u8 = fields.next().expect("stage").parse().expect("stage number");
            let mut entry = IndexEntry::new(path, oid, mode);
            entry.stage = stage;
            entries.push(entry);
        }
        let mut index = Index {
            entries,
            ..Index::default()
        };
        index.sort();
        index
    }

    /// git 对「某路径是否被 .gitignore 规则忽略」的真值。
    /// `--no-index`：不把 index 里的已跟踪状态算进来（mg 的 `Ignore` 只看规则文件）。
    fn check_ignore(&self, path: &str) -> bool {
        self.git(&["check-ignore", "--no-index", "-q", "--", path])
            .status
            .success()
    }
}

fn split_nul(raw: &[u8]) -> Vec<Vec<u8>> {
    raw.split(|b| *b == 0)
        .filter(|record| !record.is_empty())
        .map(|record| record.to_vec())
        .collect()
}

// ------------------------------------------------------------------- oracle 层

/// 被测函数：`Worktree::status(...)?.porcelain()` —— 用 git 现场构造的输入。
fn mg_porcelain_with(
    scratch: &Scratch,
    head: Option<&Tree>,
    index: &Index,
) -> Result<String, minigit::Error> {
    let repo = Repo::discover(scratch.path()).expect("discover repo");
    let worktree = Worktree::new(&repo);
    Ok(worktree.status(head, index)?.porcelain())
}

fn mg_porcelain(scratch: &Scratch) -> Result<String, minigit::Error> {
    let head = scratch.head_tree();
    let index = scratch.index();
    mg_porcelain_with(scratch, head.as_ref(), &index)
}

/// 硬标准：`porcelain()` 与真实 `git status --porcelain` 逐字节相同。
fn assert_parity(scratch: &Scratch, scenario: &str) {
    let want = scratch.truth();
    let got = match mg_porcelain(scratch) {
        Ok(got) => got,
        Err(err) => panic!("mg status errored in scenario `{scenario}`: {err:?}"),
    };
    assert_eq!(
        got, want,
        "porcelain mismatch in scenario `{scenario}`\n--- git ---\n{want:?}\n--- mg ---\n{got:?}"
    );
}

/// 场景自检：断言 **git 的输出**包含某个状态，防止「假场景」。
fn assert_scenario(truth: &str, needle: &str, scenario: &str) {
    assert!(
        truth.contains(needle),
        "scenario `{scenario}` did not produce `{needle}`; git said:\n{truth:?}"
    );
}

/// 枚举工作区里所有文件与目录（仓库相对路径）—— 只做路径枚举，
/// 用作 `git check-ignore` 的输入清单，不参与任何真值计算。
fn walk_worktree(root: &Path) -> Vec<(Vec<u8>, bool)> {
    let mut out = Vec::new();
    walk_dir(root, &mut Vec::new(), &mut out);
    out
}

fn walk_dir(dir: &Path, rel: &mut Vec<u8>, out: &mut Vec<(Vec<u8>, bool)>) {
    let mut names: Vec<std::ffi::OsString> = fs::read_dir(dir)
        .expect("read_dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    names.sort();
    for name in names {
        if name.as_bytes() == b".git" {
            continue;
        }
        let path = dir.join(&name);
        let meta = fs::symlink_metadata(&path).expect("symlink_metadata");
        let mark = rel.len();
        if !rel.is_empty() {
            rel.push(b'/');
        }
        rel.extend_from_slice(name.as_bytes());
        out.push((rel.clone(), meta.is_dir()));
        if meta.is_dir() {
            walk_dir(&path, rel, out);
        }
        rel.truncate(mark);
    }
}

// ============================================================ (B) 必测场景 1-9

/// 场景 1：已提交文件的修改 / 删除。
#[test]
fn s1_tracked_modification_and_deletion_match_git() {
    let scratch = Scratch::new();
    scratch.write("mod.txt", "one\n");
    scratch.write("del.txt", "gone\n");
    scratch.write("sub/keep.txt", "keep\n");
    scratch.commit("init");
    scratch.write("mod.txt", "two\n");
    scratch.remove("del.txt");

    let truth = scratch.truth();
    assert_scenario(&truth, " M mod.txt", "s1");
    assert_scenario(&truth, " D del.txt", "s1");
    assert_parity(&scratch, "s1-tracked-modify-delete");
}

/// 场景 2：stage 过的修改（X 列）；含「暂存后又改了工作区」→ `MM`。
#[test]
fn s2_staged_modification_matches_git() {
    let scratch = Scratch::new();
    scratch.write("staged.txt", "base\n");
    scratch.write("both.txt", "base\n");
    scratch.commit("init");
    scratch.write("staged.txt", "staged\n");
    scratch.git_ok(&["add", "staged.txt"]);
    scratch.write("both.txt", "staged\n");
    scratch.git_ok(&["add", "both.txt"]);
    scratch.write("both.txt", "worktree after staging\n");

    let truth = scratch.truth();
    assert_scenario(&truth, "M  staged.txt", "s2");
    assert_scenario(&truth, "MM both.txt", "s2");
    assert_parity(&scratch, "s2-staged-modification");
}

/// 场景 3：只 stage 的新文件（`A `）；新文件放在尚未跟踪的目录里。
#[test]
fn s3_staged_new_file_matches_git() {
    let scratch = Scratch::new();
    scratch.write("base.txt", "b\n");
    scratch.commit("init");
    scratch.write("new.txt", "n\n");
    scratch.git_ok(&["add", "new.txt"]);
    scratch.write("dir/added.txt", "a\n");
    scratch.git_ok(&["add", "dir/added.txt"]);
    scratch.write("dir/loose.txt", "l\n");

    let truth = scratch.truth();
    assert_scenario(&truth, "A  new.txt", "s3");
    assert_scenario(&truth, "A  dir/added.txt", "s3");
    assert_parity(&scratch, "s3-staged-new");
}

/// 场景 4：未跟踪文件与未跟踪目录（目录里全是未跟踪文件 → 折叠成一条 `?? dir/`）。
#[test]
fn s4_untracked_file_and_directory_folding_match_git() {
    let scratch = Scratch::new();
    scratch.write("tracked.txt", "t\n");
    scratch.commit("init");
    scratch.write("loose.txt", "l\n");
    scratch.write("dir/a.txt", "a\n");
    scratch.write("dir/sub/b.txt", "b\n");
    scratch.write("dir/sub/deep/c.txt", "c\n");
    scratch.mkdir("empty/also-empty");

    let truth = scratch.truth();
    assert_scenario(&truth, "?? loose.txt", "s4");
    assert_scenario(&truth, "?? dir/", "s4");
    assert!(
        !truth.contains("dir/a.txt"),
        "scenario s4 must fold the whole directory: {truth:?}"
    );
    assert!(
        !truth.contains("empty"),
        "scenario s4 must not report empty directories: {truth:?}"
    );
    assert_parity(&scratch, "s4-untracked-folding");
}

/// 场景 5：`.gitignore` —— 被忽略的不出现、`!` 取反救回、`build/` 目录规则。
#[test]
fn s5_gitignore_ignored_rescued_and_dir_rule_match_git() {
    let scratch = Scratch::new();
    scratch.write(".gitignore", "*.log\n!keep.log\nbuild/\n");
    scratch.write("keep.log", "keep\n");
    scratch.write("drop.log", "drop\n");
    scratch.write("build/out.txt", "out\n");
    scratch.write("build/deep/out.txt", "deep\n");
    scratch.write("visible.txt", "v\n");

    let truth = scratch.truth();
    assert_scenario(&truth, "?? keep.log", "s5");
    assert_scenario(&truth, "?? visible.txt", "s5");
    assert!(!truth.contains("drop.log"), "s5: ignored file leaked: {truth:?}");
    assert!(!truth.contains("build"), "s5: ignored dir leaked: {truth:?}");
    assert_parity(&scratch, "s5-gitignore");
}

/// 场景 6：可执行位变化（工作区侧 ` M`）。
#[test]
fn s6_worktree_exec_bit_change_matches_git() {
    let scratch = Scratch::new();
    scratch.write("run.sh", "#!/bin/sh\n");
    scratch.commit("init");
    scratch.chmod_exec("run.sh");

    let truth = scratch.truth();
    assert_scenario(&truth, " M run.sh", "s6");
    assert_parity(&scratch, "s6-exec-bit");
}

/// 场景 7：symlink —— 指向改变算 `M`。
#[test]
fn s7_symlink_target_change_matches_git() {
    let scratch = Scratch::new();
    scratch.write("one", "one\n");
    scratch.write("two", "two\n");
    scratch.symlink("one", "link");
    scratch.commit("init");
    scratch.remove("link");
    scratch.symlink("two", "link");

    // 输入侧自检：index 里的 symlink 必须是 120000（证明 mode 来自 git 而不是猜的）。
    let index = scratch.index();
    let entry = index
        .lookup(b"link")
        .expect("link in index")
        .clone();
    assert_eq!(entry.mode, FileMode::Symlink);

    let truth = scratch.truth();
    assert_scenario(&truth, " M link", "s7");
    assert_parity(&scratch, "s7-symlink-target-change");
}

/// 场景 8：无提交的仓库（`head_tree = None`）。
#[test]
fn s8_repo_without_commits_matches_git() {
    let scratch = Scratch::new();
    scratch.write("staged.txt", "s\n");
    scratch.git_ok(&["add", "staged.txt"]);
    scratch.write("untracked.txt", "u\n");
    scratch.write("dir/x.txt", "x\n");

    assert!(
        scratch.head_tree().is_none(),
        "scenario s8 must have no HEAD"
    );
    let truth = scratch.truth();
    assert_scenario(&truth, "A  staged.txt", "s8");
    assert_scenario(&truth, "?? untracked.txt", "s8");
    assert_parity(&scratch, "s8-no-commit");
}

/// 场景 9：冲突态 —— 用真实 git 造出 `UU`，再从 `ls-files --stage` 拿 stage 1/2/3。
#[test]
fn s9_merge_conflict_uu_matches_git() {
    let scratch = Scratch::new();
    scratch.write("conflict.txt", "base\n");
    scratch.write("clean.txt", "clean\n");
    scratch.commit("base");
    scratch.git_ok(&["checkout", "-q", "-b", "side"]);
    scratch.write("conflict.txt", "side\n");
    scratch.commit("side");
    scratch.git_ok(&["checkout", "-q", "main"]);
    scratch.write("conflict.txt", "main\n");
    scratch.commit("main");

    let merge = scratch.git(&["merge", "side"]);
    assert!(!merge.status.success(), "merge must conflict");

    let index = scratch.index();
    let stages: BTreeSet<u8> = index
        .entries
        .iter()
        .filter(|entry| entry.path == b"conflict.txt")
        .map(|entry| entry.stage)
        .collect();
    assert_eq!(
        stages,
        BTreeSet::from([1u8, 2, 3]),
        "scenario s9 must produce stage 1/2/3 entries"
    );
    assert!(index.has_conflicts(), "index must report conflicts");

    let truth = scratch.truth();
    assert_scenario(&truth, "UU conflict.txt", "s9");
    assert_parity(&scratch, "s9-merge-conflict-uu");
}

/// 额外场景：`git rm --cached`（索引里删掉但工作区留着）→ `D ` 与 `??` 同时出现。
#[test]
fn s10_rm_cached_matches_git() {
    let scratch = Scratch::new();
    scratch.write("z/f.txt", "z\n");
    scratch.write("stays.txt", "s\n");
    scratch.commit("init");
    scratch.git_ok(&["rm", "-q", "--cached", "z/f.txt"]);

    let truth = scratch.truth();
    assert_scenario(&truth, "D  z/f.txt", "s10");
    assert_scenario(&truth, "?? z/", "s10");
    assert_parity(&scratch, "s10-rm-cached");
}

// ============================================================ (B) 边界与反例

/// 边界：空仓库（无 HEAD、无文件）→ `porcelain()` 是空字符串。
#[test]
fn b1_empty_repo_porcelain_is_empty() {
    let scratch = Scratch::new();
    assert_eq!(scratch.truth(), "");
    assert!(scratch.head_tree().is_none());
    assert!(scratch.index().is_empty());
    assert_eq!(mg_porcelain(&scratch).expect("status"), "");
}

/// 边界：`.gitignore` 里的非法/怪异模式不得 panic（`[` 未闭合等）。
#[test]
fn b2_malformed_gitignore_patterns_never_panic() {
    let scratch = Scratch::new();
    scratch.write(".gitignore", "[\na[b\n\\\n!\n*.o\n");
    scratch.write("keep.txt", "k\n");
    scratch.write("x.o", "o\n");

    let got = mg_porcelain(&scratch).expect("malformed .gitignore must not panic / error");
    let want = scratch.truth();

    // 受支持的行（`*.o`）两边必须一致：该文件不出现。
    assert!(!got.contains("x.o"), "mg leaked ignored file: {got:?}");
    assert!(!want.contains("x.o"), "git leaked ignored file: {want:?}");
    // 未跟踪的 keep.txt 两边都必须出现。
    assert!(got.contains("keep.txt"), "mg lost keep.txt: {got:?}");
    assert!(want.contains("keep.txt"), "git lost keep.txt: {want:?}");
    println!("[probe malformed-class] git={want:?} mg={got:?}");
}

/// 边界：3 层嵌套的稀疏树 —— 折叠与排序都必须与 git 一致。
#[test]
fn b3_deep_sparse_tree_ordering_matches_git() {
    let scratch = Scratch::new();
    for rel in [
        "a.txt",
        "z.txt",
        "l1/tracked.txt",
        "l1/l2/tracked.txt",
        "l1/l2/l3/tracked.txt",
    ] {
        scratch.write(rel, &format!("{rel}\n"));
    }
    scratch.commit("init");

    scratch.write("l1/l2/l3/tracked.txt", "modified\n");
    scratch.write("l1/u.txt", "u1\n");
    scratch.write("l1/l2/u2.txt", "u2\n");
    scratch.write("l1/l2/l3/u3.txt", "u3\n");
    scratch.write("p1/p2/p3/only.txt", "deep untracked\n");
    scratch.write("d-2.txt", "dash\n");
    scratch.write("d.txt", "d\n");
    scratch.write("d/x.txt", "x\n");

    let truth = scratch.truth();
    assert_scenario(&truth, "?? p1/", "b3");
    assert_scenario(&truth, "?? l1/l2/u2.txt", "b3");
    assert_parity(&scratch, "b3-deep-sparse-tree");
}

/// 额外（独立确认作者的一个断言）：porcelain v1 是「先 changed 行、再 `??` 行」两组。
/// 场景特意让 changed 路径（`z.txt`）排在 untracked 路径（`aa.txt`）之后。
#[test]
fn b4_porcelain_groups_changed_before_untracked() {
    let scratch = Scratch::new();
    scratch.write("z.txt", "one\n");
    scratch.commit("init");
    scratch.write("z.txt", "two\n");
    scratch.write("aa.txt", "new\n");

    let truth = scratch.truth();
    let changed_at = truth.find(" M z.txt").expect("changed line");
    let untracked_at = truth.find("?? aa.txt").expect("untracked line");
    assert!(
        changed_at < untracked_at,
        "git groups changed before untracked; got {truth:?}"
    );
    assert_parity(&scratch, "b4-grouping");
}

// ============================================================ scan / ignore 直测

/// `scan_worktree` 对拍：集合来自 `git ls-files`（tracked + untracked / ignored 两个 oracle）。
#[test]
fn scan_worktree_matches_git_ls_files_oracles() {
    let scratch = Scratch::new();
    scratch.write("tracked.txt", "t\n");
    scratch.write("build/tracked.txt", "bt\n");
    scratch.write("sub/keep.txt", "k\n");
    scratch.commit("init");
    scratch.write(".gitignore", "*.log\nbuild/\n");
    scratch.write("loose.txt", "l\n");
    scratch.write("sub/new.txt", "n\n");
    scratch.write("dir/deep/a.txt", "a\n");
    scratch.write("drop.log", "ignored\n");
    scratch.mkdir("empty-dir");

    let repo = Repo::discover(scratch.path()).expect("discover repo");
    let scanned = Worktree::new(&repo).scan().expect("scan");

    let visible: BTreeSet<Vec<u8>> =
        split_nul(&scratch.git_ok(&["ls-files", "-c", "-o", "--exclude-standard", "-z"]))
            .into_iter()
            .collect();
    let ignored: BTreeSet<Vec<u8>> =
        split_nul(&scratch.git_ok(&["ls-files", "-o", "-i", "--exclude-standard", "-z"]))
            .into_iter()
            .collect();

    // 1) scan 的非忽略文件必须都在 git 的 tracked+untracked 清单里。
    let got_visible: BTreeSet<Vec<u8>> = scanned
        .iter()
        .filter(|file| !file.is_dir && !file.ignored)
        .map(|file| file.path.clone())
        .collect();
    assert!(
        got_visible.is_subset(&visible),
        "scan reported files git does not list as tracked/untracked: {:?}",
        got_visible.difference(&visible).collect::<Vec<_>>()
    );

    // 1b) 反向：git 的每个 tracked/untracked 文件都要被 scan 看到，**除非**它位于一个
    //     「整目录被忽略」的目录里 —— scan 不下降这类目录是任务书 §4.1 的设计要求
    //     （已跟踪文件落在被忽略目录里时，git 仍然列出它，status 靠 index 而非 scan 处理）。
    let pruned: Vec<Vec<u8>> = scanned
        .iter()
        .filter(|file| file.is_dir && file.ignored)
        .map(|file| {
            let mut prefix = file.path.clone();
            prefix.push(b'/');
            prefix
        })
        .collect();
    assert!(
        !pruned.is_empty(),
        "scenario must exercise ignored-directory pruning"
    );
    for path in &visible {
        let inside_pruned = pruned.iter().any(|prefix| path.starts_with(prefix));
        assert!(
            got_visible.contains(path) || inside_pruned,
            "scan missed {:?} which is not inside an ignored directory",
            String::from_utf8_lossy(path)
        );
    }

    // 2) 被忽略的文件条目必须都在 git 的 ignored 清单里。
    for file in scanned.iter().filter(|file| !file.is_dir && file.ignored) {
        assert!(
            ignored.contains(&file.path),
            "scan marked {:?} ignored but git does not",
            String::from_utf8_lossy(&file.path)
        );
    }

    // 3) 目录条目只可能是被忽略的目录，且被忽略目录**不被下降**。
    for dir in scanned.iter().filter(|file| file.is_dir) {
        assert!(dir.ignored, "non-ignored directory leaked into scan");
        let mut prefix = dir.path.clone();
        prefix.push(b'/');
        assert!(
            !scanned.iter().any(|file| file.path.starts_with(&prefix)),
            "scan descended into ignored directory {:?}",
            String::from_utf8_lossy(&dir.path)
        );
    }

    // 4) `.git` 与空目录都不出现。
    assert!(scanned.iter().all(|file| file.path != b".git"));
    assert!(scanned
        .iter()
        .all(|file| !file.path.starts_with(b"empty-dir")));

    // 5) 返回顺序是按 path 字节序排序的。
    let order: Vec<Vec<u8>> = scanned.iter().map(|file| file.path.clone()).collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "scan_worktree must return paths in byte order");
}

/// `Ignore::is_ignored` 对拍：逐路径与 `git check-ignore --no-index -q` 比较。
#[test]
fn ignore_engine_matches_git_check_ignore() {
    let scratch = Scratch::new();
    scratch.write(
        ".gitignore",
        "*.log\n!keep.log\nbuild/\n/sub/anchored.txt\ndoc/[abc].txt\n*.tmp\n",
    );
    // 子目录里的 `.gitignore`：base 作用域 + 反选 + 锚定反选。
    scratch.write("sub/.gitignore", "*.bak\n!keep.bak\n!/deep/anchored.bak\n");
    scratch.write("a.log", "a\n");
    scratch.write("keep.log", "k\n");
    scratch.write("b.tmp", "b\n");
    scratch.write("plain.txt", "p\n");
    scratch.write("sub/anchored.txt", "s\n");
    scratch.write("sub/other.txt", "o\n");
    scratch.write("sub/deep/anchored.txt", "d\n");
    scratch.write("sub/x.bak", "x\n");
    scratch.write("sub/keep.bak", "kb\n");
    scratch.write("sub/deep/anchored.bak", "ab\n");
    scratch.write("sub/deep/other.bak", "ob\n");
    scratch.write("x.bak", "root-level, must not see sub's rule\n");
    scratch.write("doc/a.txt", "da\n");
    scratch.write("doc/z.txt", "dz\n");
    scratch.write("build/x.txt", "bx\n");
    scratch.write("nested/inner.txt", "i\n");

    let repo = Repo::discover(scratch.path()).expect("discover repo");
    let ignore = Ignore::load(&repo).expect("load ignore");

    let mut checked = 0usize;
    for (path, is_dir) in walk_worktree(scratch.path()) {
        // check-ignore 只吃 UTF-8 参数；本场景所有路径都是 ASCII。
        let Some(text) = std::str::from_utf8(&path).ok().map(str::to_string) else {
            continue;
        };
        let want = scratch.check_ignore(&text);
        let got = ignore.is_ignored(&path, is_dir);
        assert_eq!(
            got, want,
            "is_ignored({text:?}, is_dir={is_dir}) = {got}, git check-ignore says {want}"
        );
        checked += 1;
    }
    assert!(checked >= 14, "expected to compare every path, saw {checked}");
}

/// §3 的防御性要求：展平 tree 里若混入 `FileMode::Tree` 条目，必须被忽略。
#[test]
fn head_tree_tree_mode_entries_are_ignored() {
    let scratch = Scratch::new();
    scratch.write("sub/keep.txt", "k\n");
    scratch.write("top.txt", "t\n");
    scratch.commit("init");
    scratch.write("sub/keep.txt", "changed\n");
    scratch.write("loose.txt", "l\n");

    let head = scratch.head_tree().expect("head tree");
    let mut with_trees = head.clone();
    for entry in head.entries() {
        // 把每个路径的每一级祖先都做成 tree 条目塞进去（模拟未展平的调用方）。
        let mut prefix = Vec::new();
        for component in entry.name.split(|b| *b == b'/') {
            if !prefix.is_empty() {
                with_trees.0.push(TreeEntry {
                    mode: FileMode::Tree,
                    name: prefix.clone(),
                    oid: entry.oid,
                });
            }
            if !prefix.is_empty() {
                prefix.push(b'/');
            }
            prefix.extend_from_slice(component);
        }
    }
    assert!(with_trees.entries().iter().any(|e| e.mode == FileMode::Tree));

    let index = scratch.index();
    let got = mg_porcelain_with(&scratch, Some(&with_trees), &index).expect("status");
    assert_eq!(
        got,
        scratch.truth(),
        "FileMode::Tree entries in head_tree must be ignored"
    );
}

// ============================================================ 反假绿（断言真实性）

/// 断言真实性：harness 必须对**故意篡改的输入**给出与 git 不同的输出。
/// 如果这里失败，说明 parity 断言是恒真的（假绿）。
#[test]
fn harness_detects_deliberately_wrong_inputs() {
    let scratch = Scratch::new();
    scratch.write("a.txt", "base\n");
    scratch.commit("init");
    scratch.write("a.txt", "changed\n");
    scratch.write("u.txt", "untracked\n");

    let want = scratch.truth();
    let index = scratch.index();
    let head = scratch.head_tree().expect("head tree");

    // 0) 正确输入 → 逐字节相同（说明它不是「恒不相同」）。
    let good = mg_porcelain_with(&scratch, Some(&head), &index).expect("status");
    assert_eq!(good, want);

    // 1) 用空 tree 冒充 HEAD：必须被发现。
    let wrong_head = Tree::default();
    let bad_head =
        mg_porcelain_with(&scratch, Some(&wrong_head), &index).expect("status with empty head");
    assert_ne!(
        bad_head, want,
        "harness failed to notice a wrong head_tree: {bad_head:?}"
    );

    // 2) 篡改 index 里 stage 0 条目的 oid：必须被发现。
    let mut bad_oid = scratch.index();
    for entry in bad_oid.entries.iter_mut() {
        if entry.path == b"a.txt" {
            entry.oid = Oid::hash_object("blob", b"definitely-not-the-content\n");
        }
    }
    let bad_oid_out =
        mg_porcelain_with(&scratch, Some(&head), &bad_oid).expect("status with wrong oid");
    assert_ne!(
        bad_oid_out, want,
        "harness failed to notice a wrong index oid: {bad_oid_out:?}"
    );

    // 3) 篡改 index 里 stage 0 条目的 mode（可执行位）：必须被发现。
    let mut bad_mode = scratch.index();
    for entry in bad_mode.entries.iter_mut() {
        if entry.path == b"a.txt" {
            entry.mode = FileMode::Executable;
        }
    }
    let bad_mode_out =
        mg_porcelain_with(&scratch, Some(&head), &bad_mode).expect("status with wrong mode");
    assert_ne!(
        bad_mode_out, want,
        "harness failed to notice a wrong index mode: {bad_mode_out:?}"
    );
}

/// 混合压力场景：把多种状态塞进同一个仓库，一次对拍。
#[test]
fn mixed_states_stress_matches_git() {
    let scratch = Scratch::new();
    scratch.write(".gitignore", "*.log\n!keep.log\nbuild/\n");
    scratch.write("staged-mode.sh", "#!/bin/sh\n");
    scratch.write("sub/tracked.txt", "t\n");
    scratch.write("gone.txt", "g\n");
    scratch.commit("init");

    // X 列：暂存模式变化 + 暂存内容变化 + 暂存新增；Y 列：内容 / 模式 / 删除；
    // `??`：散落文件与被忽略目录。注意不要在这里再跑 `git commit`（否则暂存区被清空）。
    scratch.chmod_exec("staged-mode.sh");
    scratch.git_ok(&["add", "staged-mode.sh"]);
    scratch.write("staged-mode.sh", "#!/bin/sh\necho changed\n");
    scratch.write("sub/tracked.txt", "staged\n");
    scratch.git_ok(&["add", "sub/tracked.txt"]);
    scratch.write("sub/tracked.txt", "worktree again\n");
    scratch.write("sub/fresh.txt", "f\n");
    scratch.git_ok(&["add", "sub/fresh.txt"]);
    scratch.write("sub/fresh.txt", "f changed\n");
    scratch.git_ok(&["rm", "-q", "gone.txt"]);

    scratch.write("untracked.txt", "u\n");
    scratch.write("keep.log", "rescued by the `!` rule\n");
    scratch.write("udir/a.txt", "a\n");
    scratch.write("udir/deep/b.txt", "b\n");
    scratch.write("udir/skip.log", "ignored\n");
    scratch.write("build/new.txt", "ignored\n");
    scratch.write("drop.log", "ignored\n");
    scratch.write("keep2.log", "ignored\n");

    let truth = scratch.truth();
    assert_scenario(&truth, "MM staged-mode.sh", "stress");
    assert_scenario(&truth, "MM sub/tracked.txt", "stress");
    assert_scenario(&truth, "AM sub/fresh.txt", "stress");
    assert_scenario(&truth, "D  gone.txt", "stress");
    assert_scenario(&truth, "?? udir/", "stress");
    assert_scenario(&truth, "?? keep.log", "stress");
    assert!(!truth.contains("drop.log"), "stress: ignored file leaked");
    assert!(!truth.contains("build"), "stress: ignored dir leaked");
    assert_parity(&scratch, "stress-mixed-states");
}

/// 非 UTF-8 但**未跟踪**的路径：mg 的字节级渲染应与 git 的 8 进制转义一致。
#[test]
fn non_utf8_untracked_path_matches_git() {
    let scratch = Scratch::new();
    scratch.write("tracked.txt", "t\n");
    scratch.commit("init");
    scratch.write_bytes(b"bad\xff name.txt", b"raw\n");

    let truth = scratch.truth();
    assert!(
        truth.contains("?? \"bad\\377 name.txt\""),
        "scenario must exercise git's octal quoting: {truth:?}"
    );
    assert_parity(&scratch, "non-utf8-untracked");
}

// ============================================================ 已知偏差（记录用）

/// 偏差 1（**已在 W3 修复**）：普通文件 ↔ symlink 的**类型变化**。
/// W1 时 git 输出 ` T` 而 mg 只能输出 ` M`（冻结的 `ChangeKind` 只有 A/M/D）→ 当时记为偏差。
/// W3 的 C-13 给 `ChangeKind` 加了 `TypeChanged`，T12 在 `worktree/status.rs` 里真正产出它，
/// 所以本用例现在断言**与 git 逐字节一致**（controller round，见 ORCHESTRATION.md C-19）。
#[test]
fn type_change_regular_to_symlink_matches_git() {
    let scratch = Scratch::new();
    scratch.write("a.txt", "a\n");
    scratch.write("f.txt", "same content\n");
    scratch.commit("init");
    scratch.remove("f.txt");
    scratch.write("target.txt", "same content\n");
    scratch.symlink("target.txt", "f.txt");

    let want = scratch.truth();
    let got = mg_porcelain(&scratch).expect("status");
    println!("[probe type-change] git={want:?} mg={got:?}");
    assert!(
        want.contains(" T f.txt"),
        "sanity: git must render a type change as `T`: {want:?}"
    );
    assert!(
        got.contains(" T f.txt"),
        "mg must render the type change as `T` like git does: {got:?}"
    );
    assert_eq!(want, got, "type change must be byte-identical to git");
}

/// 偏差 2：`git add -N`（intent-to-add）。git 输出 ` A <path>`，mg 把该 stage 0 条目
/// 当普通条目处理 → `AM <path>`（作者在代码注释里声明「按普通 stage 0 处理」）。
#[test]
fn deviation_intent_to_add() {
    let scratch = Scratch::new();
    scratch.write("base.txt", "base\n");
    scratch.commit("init");
    scratch.write("new.txt", "new content\n");
    scratch.git_ok(&["add", "-N", "new.txt"]);

    let want = scratch.truth();
    let got = mg_porcelain(&scratch).expect("status");
    println!("[probe intent-to-add] git={want:?} mg={got:?}");
    assert!(
        want.contains(" A new.txt"),
        "sanity: git must render intent-to-add as ` A`: {want:?}"
    );
    assert!(
        got.contains("AM new.txt"),
        "characterization: mg renders it as `AM`: {got:?}"
    );
}

/// 偏差 3：未闭合的字符类 `[`。任务书只要求「不得 panic」（已满足），
/// 但 mg 把未闭合的 `[` 当**字面量**处理，与 git（不匹配任何路径）不同 →
/// 含此类模式的仓库其 porcelain 不逐字节相同。
#[test]
fn deviation_malformed_class_is_treated_as_literal() {
    let scratch = Scratch::new();
    scratch.write(".gitignore", "[\n");
    scratch.write("[", "bracket\n");
    scratch.write("keep.txt", "k\n");

    let want = scratch.truth();
    let got = mg_porcelain(&scratch).expect("status (no panic)");
    println!("[probe malformed-class] git={want:?} mg={got:?}");
    assert!(
        want.contains("?? ["),
        "sanity: git does not match a literal `[` with an unclosed class: {want:?}"
    );
    assert!(
        !got.contains("?? ["),
        "characterization: mg treats the unclosed class as a literal `[`: {got:?}"
    );
}

/// 偏差 4：`Ignore::is_ignored` 会应用「被排除目录内部的反选规则」。
/// git 不会（父目录被排除后其内容不可能被救回）。此差异**不影响 porcelain**
/// （`scan_worktree` 不下降被忽略目录），但库层谓词与 git 不同。
#[test]
fn deviation_negation_inside_excluded_dir() {
    let scratch = Scratch::new();
    scratch.write(".gitignore", "build/\n!build/keep.txt\n");
    scratch.write("build/keep.txt", "k\n");
    scratch.write("build/other.txt", "o\n");

    let repo = Repo::discover(scratch.path()).expect("discover repo");
    let ignore = Ignore::load(&repo).expect("load ignore");
    let mg = ignore.is_ignored(b"build/keep.txt", false);
    let git = scratch.check_ignore("build/keep.txt");
    println!("[probe negation-in-excluded-dir] git={git} mg={mg}");
    assert!(git, "sanity: git keeps it ignored: {git}");
    assert!(
        !mg,
        "characterization: mg applies the root-level negation anyway: {mg}"
    );

    // porcelain 层面仍然一致（scan 不下降被忽略目录）。
    assert_parity(&scratch, "deviation-4-porcelain-unaffected");
}

/// 偏差 5：**已跟踪**的非 UTF-8 路径。`Repo::work_path`（冻结、controller-owned）
/// 只接受 UTF-8 → `status()` 返回 Err，而不是像 git 那样用 8 进制转义渲染。
#[test]
fn deviation_tracked_non_utf8_path_is_unsupported() {
    let scratch = Scratch::new();
    scratch.write_bytes(b"bad\xff.txt", b"raw\n");
    scratch.git_ok(&["add", "-A"]);
    scratch.git_ok(&["commit", "-q", "-m", "non-utf8"]);
    // 让工作区再次变脏，否则 git 的输出是空的。
    scratch.write_bytes(b"bad\xff.txt", b"changed\n");

    let want = scratch.truth();
    assert!(
        !want.is_empty() && want.contains("bad\\377.txt"),
        "sanity: git must report the modified non-UTF-8 path: {want:?}"
    );
    let got = mg_porcelain(&scratch);
    println!("[probe tracked-non-utf8] git={want:?} mg={got:?}");
    assert!(
        got.is_err(),
        "characterization: mg must fail loudly (frozen Repo::work_path rejects non-UTF-8)"
    );
}
