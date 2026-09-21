//! V2 —— 独立验证 T2（refs 层）。**真值只来自运行时真实 `git` 二进制**。
//!
//! 规则：本文件不出现任何手抄的 oid / refname / packed-refs 文本；
//! 所有期望值都在运行时由 `git show-ref` / `for-each-ref` / `symbolic-ref` /
//! `rev-parse` / `pack-refs` / `commit-tree` 产出，仓库一律建在 `tempfile::tempdir()` 里。
//! `git` 不可用 = 直接 panic（不静默跳过，避免假绿）。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use minigit::refs::{Head, RefStore};
use minigit::{Error, Oid, Repo};

// 这些是**自造 fixture 的名字**（不是真值）；任何断言里的 oid 都由 git 产出。
const BRANCH_WITH_SLASH: &str = "feature/x";
const BRANCH_VICTIM: &str = "zeta";
const NEW_BRANCH_SHORT: &str = "made-by-refstore";
const NEW_BRANCH: &str = "refs/heads/made-by-refstore";

// ---------------------------------------------------------------- git 测试床

fn git_raw(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "V2")
        .env("GIT_AUTHOR_EMAIL", "v2@example.com")
        .env("GIT_COMMITTER_NAME", "V2")
        .env("GIT_COMMITTER_EMAIL", "v2@example.com")
        .env("GIT_AUTHOR_DATE", "1700000000 +0800")
        .env("GIT_COMMITTER_DATE", "1700000000 +0800")
        .args(args)
        .output()
        .expect("failed to spawn git")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_raw(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("git stdout is UTF-8")
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    git_raw(dir, args).status.success()
}

/// 真值来源：`git rev-parse <rev>`。
fn oid_of(dir: &Path, rev: &str) -> Oid {
    let hex = git(dir, &["rev-parse", rev]);
    Oid::from_hex(hex.trim()).unwrap_or_else(|e| panic!("git rev-parse {rev} printed {hex:?}: {e}"))
}

/// 真值来源：`git show-ref`（`<oid> <refname>`，已排序）。
fn show_ref_truth(dir: &Path) -> Vec<(String, Oid)> {
    let mut out: Vec<(String, Oid)> = git(dir, &["show-ref"])
        .lines()
        .map(|line| {
            let (hex, name) = line
                .split_once(' ')
                .unwrap_or_else(|| panic!("unexpected show-ref line {line:?}"));
            (
                name.trim().to_string(),
                Oid::from_hex(hex).unwrap_or_else(|e| panic!("show-ref oid {hex:?}: {e}")),
            )
        })
        .collect();
    out.sort();
    out
}

/// 真值来源：`git for-each-ref --format='%(refname:short)' refs/heads/`。
fn heads_truth(dir: &Path) -> Vec<String> {
    git(
        dir,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    )
    .lines()
    .map(str::to_string)
    .collect()
}

fn ref_exists(dir: &Path, refname: &str) -> bool {
    git_ok(dir, &["show-ref", "--verify", "--quiet", refname])
}

/// 让真实 git 造一个**没有被任何 ref 引用**的新 commit oid；mg 侧不许手写 oid。
fn fresh_commit_oid(dir: &Path) -> Oid {
    let tree = git(dir, &["rev-parse", "refs/heads/main^{tree}"]);
    let hex = git(dir, &["commit-tree", tree.trim(), "-m", "v2-verify-dangling"]);
    Oid::from_hex(hex.trim()).unwrap_or_else(|e| panic!("commit-tree printed {hex:?}: {e}"))
}

fn open(dir: &Path) -> Repo {
    Repo::discover(dir).expect("minigit must discover the fixture repo")
}

/// `git init` + 1 次提交 + 2 个分支 + 1 个带注解 tag + 1 个轻量 tag。
fn fixture() -> tempfile::TempDir {
    assert!(
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
        "real git binary is required for this verification target"
    );
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    git(dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("a.txt"), "hello\n").expect("write a.txt");
    git(dir, &["add", "--", "a.txt"]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    git(dir, &["branch", BRANCH_WITH_SLASH]);
    git(dir, &["branch", BRANCH_VICTIM]);
    git(dir, &["tag", "-a", "v1", "-m", "annotated tag"]);
    git(dir, &["tag", "light"]);
    tmp
}

fn loose_path(dir: &Path, refname: &str) -> PathBuf {
    dir.join(".git").join(refname)
}

// ---------------------------------------------------------------- 用例

/// 验收 1（读方向）：`list()` / `branches()` / `packed()` / `read_head()` 与真实 git 逐条相同。
#[test]
fn read_direction_matches_real_git_after_pack_refs() {
    let tmp = fixture();
    let dir = tmp.path();
    let truth = show_ref_truth(dir);
    let heads = heads_truth(dir);
    assert!(truth.len() >= 5, "fixture must have >=5 refs: {truth:?}");

    git(dir, &["pack-refs", "--all"]);
    assert!(
        dir.join(".git/packed-refs").is_file(),
        "git did not write packed-refs"
    );
    // 至少有一条 ref 真的只剩 packed 形态，否则本用例没在考 packed 读取。
    let packed_only: Vec<&(String, Oid)> = truth
        .iter()
        .filter(|(name, _)| !loose_path(dir, name).exists())
        .collect();
    assert!(
        !packed_only.is_empty(),
        "fixture: no ref was actually packed, cannot exercise packed-refs"
    );

    let repo = open(dir);
    let store = RefStore::new(&repo);

    let listed = store.list().expect("RefStore::list");
    assert_eq!(
        listed, truth,
        "list() must equal the parse of `git show-ref` (name+oid, sorted)"
    );

    assert_eq!(
        store.branches().expect("RefStore::branches"),
        heads,
        "branches() must equal `git for-each-ref --format='%(refname:short)' refs/heads/`"
    );

    let packed = store.packed().expect("RefStore::packed");
    assert!(
        !packed.is_empty(),
        "packed() is empty although .git/packed-refs exists"
    );
    let mut packed_sorted = packed.clone();
    packed_sorted.sort();
    let mut expected_packed: Vec<(String, Oid)> =
        packed_only.into_iter().cloned().collect();
    expected_packed.sort();
    assert_eq!(
        packed_sorted, expected_packed,
        "packed() must be exactly the packed-refs entries (no `^peeled` line leaking in)"
    );
    for entry in &packed {
        assert!(
            listed.contains(entry),
            "packed() entry {entry:?} is not a subset of list()"
        );
    }

    // HEAD（此处仍是 attached）。
    let symbolic = git(dir, &["symbolic-ref", "HEAD"]).trim().to_string();
    assert_eq!(
        store.read_head().expect("read_head"),
        Head::Attached(symbolic.clone()),
        "read_head() must equal `git symbolic-ref HEAD`"
    );
    let abbrev = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .trim()
        .to_string();
    assert_eq!(
        store.read_head().unwrap().branch_name(),
        Some(abbrev.as_str()),
        "branch_name() must match `git rev-parse --abbrev-ref HEAD`"
    );
}

/// 验收 2（写方向）：mg 写出的 ref/HEAD 必须被真实 git 读出来。
#[test]
fn write_direction_is_readable_by_real_git() {
    let tmp = fixture();
    let dir = tmp.path();
    let repo = open(dir);
    let store = RefStore::new(&repo);

    let target = oid_of(dir, "HEAD");
    let main_ref = git(dir, &["symbolic-ref", "HEAD"]).trim().to_string();

    store
        .update(NEW_BRANCH, target, None)
        .expect("creating a new branch must succeed");
    assert_eq!(
        oid_of(dir, NEW_BRANCH_SHORT),
        target,
        "`git rev-parse {NEW_BRANCH_SHORT}` must equal the written oid"
    );
    let branch_list = git(dir, &["branch", "--format=%(refname:short)"]);
    assert!(
        branch_list.lines().any(|l| l == NEW_BRANCH_SHORT),
        "`git branch` does not list the new branch:\n{branch_list}"
    );
    let expected_line = format!("{} {NEW_BRANCH}", target.to_hex());
    assert!(
        git(dir, &["show-ref"]).lines().any(|l| l == expected_line),
        "`git show-ref` does not show the new branch"
    );

    // 磁盘形态：40 位小写 hex + '\n'，且没有残留 lock 文件。
    let raw = fs::read_to_string(loose_path(dir, NEW_BRANCH)).expect("loose ref file");
    assert_eq!(raw, format!("{}\n", target.to_hex()), "loose ref body");
    assert!(
        !loose_path(dir, &format!("{NEW_BRANCH}.lock")).exists(),
        "update() left a stale .lock file"
    );

    // attached HEAD：短名与全名都要能被 git 读回。
    store
        .set_head_attached(NEW_BRANCH_SHORT)
        .expect("set_head_attached(short)");
    assert_eq!(
        git(dir, &["symbolic-ref", "HEAD"]).trim(),
        NEW_BRANCH,
        "git must read the attached HEAD written from a short name"
    );
    assert_eq!(oid_of(dir, "HEAD"), target);

    store
        .set_head_attached(&main_ref)
        .expect("set_head_attached(full name)");
    assert_eq!(git(dir, &["symbolic-ref", "HEAD"]).trim(), main_ref);

    // detached HEAD。
    store.set_head_detached(target).expect("set_head_detached");
    assert!(
        !git_ok(dir, &["symbolic-ref", "HEAD"]),
        "`git symbolic-ref HEAD` must fail on a detached HEAD"
    );
    assert_eq!(
        oid_of(dir, "HEAD"),
        target,
        "`git rev-parse HEAD` must equal the detached oid"
    );
    let head = store.read_head().expect("read_head");
    assert_eq!(head, Head::Detached(target));
    assert_eq!(head.branch_name(), None, "detached HEAD has no branch");
}

/// 验收 3（CAS）：错误 expected 必须 RefConflict，且磁盘不动。
#[test]
fn cas_conflicts_leave_disk_untouched() {
    let tmp = fixture();
    let dir = tmp.path();
    let repo = open(dir);
    let store = RefStore::new(&repo);

    let current = oid_of(dir, "refs/heads/main");
    let other = fresh_commit_oid(dir);
    assert_ne!(other, current, "fixture needs two distinct commit oids");
    let before = git(dir, &["show-ref"]);

    // 1) 错误 expected（值是另一个真实 commit）。
    let err = store
        .update("refs/heads/main", other, Some(Some(other)))
        .expect_err("wrong expected must conflict");
    assert!(
        matches!(&err, Error::RefConflict { name, .. } if name == "refs/heads/main"),
        "expected RefConflict(names refs/heads/main), got {err:?}"
    );
    assert_eq!(
        oid_of(dir, "refs/heads/main"),
        current,
        "the ref moved even though the CAS failed"
    );
    assert_eq!(git(dir, &["show-ref"]), before, "disk changed on a failed CAS");

    // 2) 已存在时用 expected=None 创建。
    let err = store
        .update("refs/heads/main", other, None)
        .expect_err("expected=None means create, must conflict on an existing ref");
    assert!(
        matches!(err, Error::RefConflict { .. }),
        "expected RefConflict, got {err:?}"
    );
    assert_eq!(oid_of(dir, "refs/heads/main"), current);

    // 2b) Some(None) 同样是「创建」语义。
    let err = store
        .update("refs/heads/main", other, Some(None))
        .expect_err("Some(None) means create, must conflict on an existing ref");
    assert!(
        matches!(err, Error::RefConflict { .. }),
        "expected RefConflict, got {err:?}"
    );
    assert_eq!(oid_of(dir, "refs/heads/main"), current);
    assert_eq!(git(dir, &["show-ref"]), before);

    // 3) 正确 expected 必须成功，并被 git 读到。
    store
        .update("refs/heads/main", other, Some(Some(current)))
        .expect("CAS with the matching expected value must succeed");
    assert_eq!(oid_of(dir, "refs/heads/main"), other);
    assert!(
        git(dir, &["show-ref"])
            .lines()
            .any(|l| l == format!("{} refs/heads/main", other.to_hex())),
        "git does not see the CAS result"
    );
}

/// 验收 4（delete × packed）：删 packed 里的条目后，真实 git 也必须看不见它。
#[test]
fn delete_removes_packed_entry_and_git_agrees() {
    let tmp = fixture();
    let dir = tmp.path();
    git(dir, &["pack-refs", "--all"]);
    assert!(
        !loose_path(dir, BRANCH_VICTIM).exists(),
        "fixture: {BRANCH_VICTIM} should only exist in packed-refs"
    );
    let repo = open(dir);
    let store = RefStore::new(&repo);

    let before = show_ref_truth(dir);
    assert!(before.iter().any(|(n, _)| n.ends_with(BRANCH_VICTIM)));
    let packed_before = fs::read_to_string(dir.join(".git/packed-refs")).expect("packed-refs");
    let victim_ref = format!("refs/heads/{BRANCH_VICTIM}");

    store
        .delete(&victim_ref)
        .expect("deleting a packed-only ref must succeed");

    let after = git(dir, &["show-ref"]);
    assert!(
        !after.lines().any(|l| l.ends_with(&victim_ref)),
        "`git show-ref` still lists the deleted ref:\n{after}"
    );
    assert!(
        !git_ok(dir, &["rev-parse", "--verify", &victim_ref]),
        "`git rev-parse --verify` still resolves the deleted ref"
    );
    assert!(!store.exists(&victim_ref), "exists() after delete()");
    let err = store
        .delete(&victim_ref)
        .expect_err("deleting a missing ref must be an error");
    assert!(
        matches!(err, Error::RefNotFound(_)),
        "deleting a missing ref must be RefNotFound, got {err:?}"
    );

    // packed-refs 重写必须精确删除那一行、其余字节（含 `^peeled` 行）原样保留。
    let packed_after = fs::read_to_string(dir.join(".git/packed-refs")).expect("packed-refs");
    let expected_rewrite: String = packed_before
        .lines()
        .filter(|l| !l.ends_with(&victim_ref))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(
        packed_after, expected_rewrite,
        "packed-refs rewrite must drop exactly the deleted entry"
    );
    let mut expected_remaining: Vec<(String, Oid)> = before.clone();
    expected_remaining.retain(|(n, _)| n != &victim_ref);
    assert_eq!(
        show_ref_truth(dir),
        expected_remaining,
        "packed-refs rewrite lost or mangled other entries"
    );

    // loose 删除路径。
    let loose_branch = "loose-branch";
    git(dir, &["branch", loose_branch]);
    let loose_ref = format!("refs/heads/{loose_branch}");
    assert!(loose_path(dir, &loose_ref).is_file());
    store.delete(&loose_ref).expect("deleting a loose ref");
    assert!(!loose_path(dir, &loose_ref).exists());
    assert!(!ref_exists(dir, &loose_ref));
}

/// 验收 5（损坏输入）：乱码 / 空 / 空符号引用 → Corrupt；文件缺失 → RefNotFound。
#[test]
fn corrupt_and_missing_head_are_typed_errors() {
    let tmp = fixture();
    let dir = tmp.path();
    let repo = open(dir);
    let store = RefStore::new(&repo);
    let head_path = dir.join(".git/HEAD");
    let symbolic_before = git(dir, &["symbolic-ref", "HEAD"]).trim().to_string();
    let good = fs::read_to_string(&head_path).expect("read HEAD");

    for garbage in [&b"!!! not a ref !!!\n"[..], b"", b"ref:\n"] {
        fs::write(&head_path, garbage).expect("write garbage HEAD");
        let err = store
            .read_head()
            .expect_err("a malformed HEAD must be an error, not a panic");
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "HEAD = {:?} must be Error::Corrupt, got {err:?}",
            String::from_utf8_lossy(garbage)
        );
    }

    // 恢复到合法内容后必须能读回（证明上面的用例没把仓库搞坏）。
    fs::write(&head_path, &good).expect("restore HEAD");
    assert_eq!(
        store.read_head().expect("read_head after restore"),
        Head::Attached(symbolic_before.clone())
    );

    fs::remove_file(&head_path).expect("remove HEAD");
    let err = store
        .read_head()
        .expect_err("a missing HEAD must be an error");
    assert!(
        matches!(&err, Error::RefNotFound(n) if n == "HEAD"),
        "missing HEAD must be Error::RefNotFound(\"HEAD\"), got {err:?}"
    );
    // 收尾：HEAD 复原后真实 git 也必须能读回。
    fs::write(&head_path, &good).expect("restore HEAD");
    assert_eq!(git(dir, &["symbolic-ref", "HEAD"]).trim(), symbolic_before);
}

/// 规格 3.3 追加：loose 覆盖 packed（必须用**不同的 oid** 才能证明优先级）。
#[test]
fn loose_ref_overrides_packed_entry_like_real_git() {
    let tmp = fixture();
    let dir = tmp.path();
    git(dir, &["pack-refs", "--all"]);
    let victim_ref = format!("refs/heads/{BRANCH_VICTIM}");
    let packed_oid = oid_of(dir, &victim_ref);
    let repo = open(dir);
    let store = RefStore::new(&repo);

    let fresh = fresh_commit_oid(dir);
    assert_ne!(fresh, packed_oid);
    store
        .update(&victim_ref, fresh, Some(Some(packed_oid)))
        .expect("CAS against a packed-only ref must succeed");

    assert_eq!(
        oid_of(dir, &victim_ref),
        fresh,
        "real git must read the loose override"
    );
    let listed = store.list().expect("list");
    assert_eq!(
        listed
            .iter()
            .find(|(n, _)| n == &victim_ref)
            .map(|(_, o)| *o),
        Some(fresh),
        "list() must let the loose value override packed"
    );
    assert!(
        store
            .packed()
            .expect("packed")
            .iter()
            .any(|(n, o)| n == &victim_ref && *o == packed_oid),
        "packed() must still report the packed value"
    );
}

/// 规格 3.5 追加：`<ref>.lock` 已存在时必须 RefConflict（并发写者）。
#[test]
fn update_refuses_while_lock_file_exists() {
    let tmp = fixture();
    let dir = tmp.path();
    let repo = open(dir);
    let store = RefStore::new(&repo);
    let target = oid_of(dir, "HEAD");

    let lock = loose_path(dir, &format!("{NEW_BRANCH}.lock"));
    fs::create_dir_all(lock.parent().expect("lock parent")).expect("create refs dir");
    fs::write(&lock, b"").expect("create lock");

    let err = store
        .update(NEW_BRANCH, target, None)
        .expect_err("a held lock must refuse the write");
    assert!(
        matches!(err, Error::RefConflict { .. }),
        "expected RefConflict while a lock is held, got {err:?}"
    );
    assert!(
        !ref_exists(dir, NEW_BRANCH),
        "the ref was created even though the lock refused the write"
    );

    fs::remove_file(&lock).expect("release lock");
    store
        .update(NEW_BRANCH, target, None)
        .expect("after the lock is gone the write must succeed");
    assert!(ref_exists(dir, NEW_BRANCH));
    assert!(!lock.exists(), "successful update left a stale lock");
}

/// 规格 3.4：已文档化的 rev 形式与真实 git 一致（斜杠短名见下一个用例的说明）。
#[test]
fn resolve_matches_git_for_documented_forms() {
    let tmp = fixture();
    let dir = tmp.path();
    let repo = open(dir);
    let store = RefStore::new(&repo);

    for rev in ["HEAD", "refs/heads/main", "refs/tags/light", "refs/tags/v1"] {
        assert_eq!(
            store.resolve(rev).expect("resolve {rev}"),
            oid_of(dir, rev),
            "resolve({rev:?}) must equal `git rev-parse {rev}`"
        );
    }
    // 短分支名 / 短 tag 名。
    for (short, full) in [("main", "refs/heads/main"), ("light", "refs/tags/light")] {
        assert_eq!(
            store.resolve(short).expect("resolve short name"),
            oid_of(dir, full),
            "resolve({short:?}) must equal `git rev-parse {full}`"
        );
    }

    // 完整 40 位 hex：直接当 oid，**不要求对象存在**（规格 3.4）。
    // 注意 `git rev-parse --verify <40hex>` 不查对象存在性，必须用 `git cat-file -e` 证伪。
    let absent = Oid::hash_object("blob", b"v2-verify-absent-object");
    assert!(
        !git_ok(dir, &["cat-file", "-e", &absent.to_hex()]),
        "fixture: the probe oid unexpectedly exists in this repo"
    );
    assert_eq!(store.resolve(&absent.to_hex()).expect("resolve 40-hex"), absent);

    for missing in ["no-such-rev", "refs/heads/no-such-ref", "no-such-tag"] {
        let err = store
            .resolve(missing)
            .expect_err("a missing rev must be an error");
        assert!(
            matches!(&err, Error::RefNotFound(n) if n == missing),
            "resolve({missing:?}) must be RefNotFound, got {err:?}"
        );
    }

    assert!(store.exists("refs/heads/main"));
    assert!(!store.exists("refs/heads/no-such-ref"));
}

/// 规格 3.4 的**文字歧义**（not part of the 5 项验收）：`feature/x` 这类含 `/` 的短名。
/// 本用例只断言 git 侧真值，并把 mg 侧的观察打印出来供 controller 判定，
/// 不把可疑偏差伪装成 pass（也不让门禁变红）。
#[test]
fn note_slash_named_branch_resolution() {
    let tmp = fixture();
    let dir = tmp.path();
    let repo = open(dir);
    let store = RefStore::new(&repo);

    let git_oid = oid_of(dir, BRANCH_WITH_SLASH);
    assert_eq!(
        git_oid,
        oid_of(dir, "HEAD"),
        "`git rev-parse {BRANCH_WITH_SLASH}` must resolve to the branch tip"
    );
    match store.resolve(BRANCH_WITH_SLASH) {
        Ok(oid) => println!(
            "note: mg resolve({BRANCH_WITH_SLASH:?}) = {oid} (git: {git_oid}) — 与 git 一致"
        ),
        Err(err) => println!(
            "note: mg resolve({BRANCH_WITH_SLASH:?}) = Err({err}) 而 `git rev-parse {BRANCH_WITH_SLASH}` = {git_oid}\
             — 规格 3.4「含 `/` 的全名」未回退到 refs/heads/<name>（真实 git 会回退）"
        ),
    }
    assert_eq!(
        store.resolve(&format!("refs/heads/{BRANCH_WITH_SLASH}")).unwrap(),
        git_oid,
        "全名形式必须永远可用"
    );
}
