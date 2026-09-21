//! V6b 独立复验：T6b —— `mg diff` 在含空格路径上的 `--- `/`+++ ` 行 TAB 修复。
//!
//! 真值 100% 来自**运行时的真实 git 进程**（不硬编码任何期望字节）：
//! * 引擎级：`git diff --no-index --no-renames --unified=<n>`，label 也从 git 输出里取回；
//! * CLI 级：`tempfile` 真实仓库 + `git diff --no-renames [--staged|HEAD]`。
//!
//! 被测对象必须真的被调用：`minigit::diff::unified::unified_bytes`（CLI 用的引擎入口）
//! 与 `env!("CARGO_BIN_EXE_mg")`（真实二进制）。git 的输出只作 oracle，不回灌成被测返回值。

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

const OLD: &[u8] = b"line1\nline2\nline3\n";
const NEW: &[u8] = b"line1\nCHANGED\nline3\n";

// ---------------------------------------------------------------------------
// 真实 git / mg（配置只作用于子进程，不 export）
// ---------------------------------------------------------------------------

fn base_git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "diff.renames=false",
            "-c",
            "core.quotePath=true",
        ])
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn git {args:?}: {err}"))
}

fn git_ok(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = base_git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: status={:?} stderr={}",
        out.status,
        esc(&out.stderr)
    );
    out.stdout
}

fn git_no_index(dir: &Path, ctx: usize, a: &[u8], b: &[u8]) -> Vec<u8> {
    // `--no-index` 有差异时退出码是 1，所以不能断言 success，只看 stdout。
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "diff.renames=false",
            "-c",
            "core.quotePath=true",
            "diff",
            "--no-index",
            "--no-renames",
            &format!("--unified={ctx}"),
            "--",
        ])
        .arg(as_os(a))
        .arg(as_os(b))
        .output()
        .expect("failed to spawn git diff --no-index");
    out.stdout
}

fn mg_run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mg"))
        .current_dir(dir)
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn mg {args:?}: {err}"))
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn as_os(bytes: &[u8]) -> &OsStr {
    OsStr::from_bytes(bytes)
}

#[cfg(not(unix))]
fn as_os(bytes: &[u8]) -> &OsStr {
    OsStr::new(std::str::from_utf8(bytes).expect("non-UTF-8 names are unix-only"))
}

fn path_in(dir: &Path, rel: &[u8]) -> PathBuf {
    dir.join(as_os(rel))
}

fn write_rel(root: &Path, rel: &[u8], bytes: &[u8]) {
    let path = path_in(root, rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, bytes).expect("write fixture");
}

fn esc(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).escape_debug().to_string()
}

fn init_repo(dir: &Path) {
    git_ok(dir, &["init", "-q", "-b", "main"]);
}

fn commit_all(dir: &Path, message: &str) {
    git_ok(dir, &["add", "-A"]);
    git_ok(
        dir,
        &[
            "-c",
            "user.name=V6b",
            "-c",
            "user.email=v6b@example.invalid",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

fn assert_same_bytes(label: &str, want: &[u8], got: &[u8]) {
    if want == got {
        return;
    }
    let want_lines: Vec<&[u8]> = want.split_inclusive(|&b| b == b'\n').collect();
    let got_lines: Vec<&[u8]> = got.split_inclusive(|&b| b == b'\n').collect();
    let mut msg = format!(
        "{label}: byte mismatch (git {} bytes, mg {} bytes)",
        want.len(),
        got.len()
    );
    for i in 0..want_lines.len().max(got_lines.len()) {
        let w = want_lines.get(i).copied().unwrap_or(b"<missing>");
        let g = got_lines.get(i).copied().unwrap_or(b"<missing>");
        if w != g {
            msg.push_str(&format!(
                "\n  first differing line #{i}:\n    git: {}\n    mg : {}",
                esc(w),
                esc(g)
            ));
            break;
        }
    }
    panic!("{msg}");
}

/// 丢掉 `diff --git` / `index` / mode 行，返回「从 `--- ` 行开始」的字节。
fn filepair_body(raw: &[u8]) -> Vec<u8> {
    let mut offset = 0usize;
    for line in raw.split_inclusive(|byte| *byte == b'\n') {
        if line.starts_with(b"--- ") {
            return raw[offset..].to_vec();
        }
        offset += line.len();
    }
    Vec::new()
}

/// 从真实 git 的输出里取回它自己写出的两个 label（去掉行尾补的 TAB）。
fn git_labels(raw: &[u8]) -> (String, String) {
    let body = filepair_body(raw);
    let lines: Vec<&[u8]> = body.split_inclusive(|b| *b == b'\n').collect();
    assert!(lines.len() >= 2, "no filepair lines in git output");
    let strip = |line: &[u8]| {
        let raw_line = line.strip_suffix(b"\n").unwrap_or(line);
        let text = raw_line.strip_suffix(b"\t").unwrap_or(raw_line);
        assert!(
            text.starts_with(b"--- ") || text.starts_with(b"+++ "),
            "not a filepair line: {}",
            esc(text)
        );
        String::from_utf8(text[4..].to_vec()).expect("label is ASCII once C-quoted")
    };
    (strip(lines[0]), strip(lines[1]))
}

/// 在**任意一侧**（git 或 mg）的输出上断言 T6b 的两条规则：
/// * `diff --git` 行**从不**补 TAB；
/// * `--- `/`+++ ` 行的名字**当且仅当**含空格字节时补一个行尾 TAB。
///
/// 名字矩阵同时包含「含空格」与「不含空格（但含 TAB/引号/CJK/非 ASCII）」两类，
/// 所以这个断言对「从不补」和「总是补」两种偏离方向都有区分力。
fn assert_padding_rules(raw: &[u8], who: &str, label: &str) {
    let lines: Vec<&[u8]> = raw.split_inclusive(|b| *b == b'\n').collect();
    assert!(!lines.is_empty(), "{label}/{who}: empty output");
    assert!(
        lines[0].starts_with(b"diff --git "),
        "{label}/{who}: first line is not `diff --git `: {}",
        esc(lines[0])
    );
    assert!(
        !lines[0].contains(&b'\t'),
        "{label}/{who}: `diff --git` line must NEVER be padded: {}",
        esc(lines[0])
    );

    let body = filepair_body(raw);
    let bl: Vec<&[u8]> = body.split_inclusive(|b| *b == b'\n').collect();
    assert!(
        bl.len() >= 2 && bl[1].starts_with(b"+++ "),
        "{label}/{who}: expected `--- ` then `+++ `, got {}",
        esc(&body)
    );
    for line in [bl[0], bl[1]] {
        let raw_line = line.strip_suffix(b"\n").unwrap_or(line);
        let (text, padded) = match raw_line.strip_suffix(b"\t") {
            Some(stripped) => (stripped, true),
            None => (raw_line, false),
        };
        let name = &text[4..];
        assert_eq!(
            padded,
            name.contains(&b' '),
            "{label}/{who}: name {:?} padded={padded}",
            String::from_utf8_lossy(name)
        );
    }
}

/// `mg` 与真实 git 逐字节比较；两侧都做规则断言，并返回 git 的 stdout。
fn compare_cli(dir: &Path, mg_args: &[&str], git_args: &[&str], label: &str) -> Vec<u8> {
    let got = mg_run(dir, mg_args);
    assert!(
        got.status.success(),
        "{label}: mg {mg_args:?} exited {:?}; stderr={}",
        got.status,
        esc(&got.stderr)
    );
    let want = base_git(dir, git_args);
    assert!(
        want.status.success(),
        "{label}: git {git_args:?} exited {:?}; stderr={}",
        want.status,
        esc(&want.stderr)
    );
    assert!(
        !want.stdout.is_empty() && !filepair_body(&want.stdout).is_empty(),
        "{label}: oracle is empty — the case is not actually being exercised"
    );
    assert_padding_rules(&want.stdout, "git", label);
    assert_padding_rules(&got.stdout, "mg", label);
    assert_same_bytes(label, &want.stdout, &got.stdout);
    want.stdout
}

// ---------------------------------------------------------------------------
// 名字矩阵
// ---------------------------------------------------------------------------

/// CLI 级名字矩阵。**不含非 UTF-8**：`mg v1` 对非 UTF-8 路径直接 fatal
/// （见 `cli_non_utf8_path_is_a_known_v1_limitation`），那一类只在引擎级覆盖。
fn wide_names() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("no space", b"plain_a.txt".to_vec()),
        ("space", b"a b.txt".to_vec()),
        ("multi space", b"a  b  c.txt".to_vec()),
        ("dir component with space", b"dir x/f.txt".to_vec()),
        ("trailing space", b"trail .txt".to_vec()),
        ("double quote", b"qu\"o_a.txt".to_vec()),
        ("backslash", b"ba\\ck_a.txt".to_vec()),
        ("tab", b"ta\tb_a.txt".to_vec()),
        ("cjk", "中文_a.txt".as_bytes().to_vec()),
        ("non-ascii arrow", "→a_a.txt".as_bytes().to_vec()),
        ("space + quote", b"q\"uo te_a.txt".to_vec()),
        ("space + tab", b"sp ace\tb_a.txt".to_vec()),
        ("space + backslash", b"sp\\ ace_a.txt".to_vec()),
        ("space + cjk", "空 格_a.txt".as_bytes().to_vec()),
    ]
}

/// 引擎级对拍用的名字对：同一类里两个不同名字，内容 OLD / NEW。
fn engine_name_pairs() -> Vec<(&'static str, Vec<u8>, Vec<u8>)> {
    let mut v: Vec<(&'static str, Vec<u8>, Vec<u8>)> = vec![
        ("no space", b"plain_a.txt".to_vec(), b"plain_b.txt".to_vec()),
        ("space", b"a b.txt".to_vec(), b"c d.txt".to_vec()),
        (
            "dir component with space",
            b"dir x/old.txt".to_vec(),
            b"dir x/new.txt".to_vec(),
        ),
        ("trailing space", b"trail_a .txt".to_vec(), b"trail_b .txt".to_vec()),
        ("double quote", b"qu\"o_a.txt".to_vec(), b"qu\"o_b.txt".to_vec()),
        ("backslash", b"ba\\ck_a.txt".to_vec(), b"ba\\ck_b.txt".to_vec()),
        ("tab", b"ta\tb_a.txt".to_vec(), b"ta\tb_b.txt".to_vec()),
        ("cjk", "中文_a.txt".as_bytes().to_vec(), "中文_b.txt".as_bytes().to_vec()),
        (
            "non-ascii arrow",
            "→a_a.txt".as_bytes().to_vec(),
            "→a_b.txt".as_bytes().to_vec(),
        ),
        (
            "space + quote",
            b"q\"uo te_a.txt".to_vec(),
            b"q\"uo te_b.txt".to_vec(),
        ),
        (
            "space + tab",
            b"sp ace\tb_a.txt".to_vec(),
            b"sp ace\tb_b.txt".to_vec(),
        ),
        (
            "space + cjk",
            "空 格_a.txt".as_bytes().to_vec(),
            "空 格_b.txt".as_bytes().to_vec(),
        ),
    ];
    #[cfg(unix)]
    {
        v.push(("non-utf-8", b"raw\xff_a.txt".to_vec(), b"raw\xff_b.txt".to_vec()));
        v.push((
            "non-utf-8 + space",
            b"raw\xff a.txt".to_vec(),
            b"raw\xff b.txt".to_vec(),
        ));
    }
    v
}

// ---------------------------------------------------------------------------
// (B) 引擎级：`unified_bytes` vs `git diff --no-index --no-renames`
// ---------------------------------------------------------------------------

#[test]
fn engine_filepair_lines_match_real_git_across_name_matrix() {
    for (class, name_a, name_b) in engine_name_pairs() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_rel(dir.path(), &name_a, OLD);
        write_rel(dir.path(), &name_b, NEW);

        for ctx in [0usize, 3] {
            let raw = git_no_index(dir.path(), ctx, &name_a, &name_b);
            assert!(
                raw.starts_with(b"diff --git "),
                "{class} -U{ctx}: no `diff --git` header: {}",
                esc(&raw)
            );
            assert_padding_rules(&raw, "git", &format!("{class} -U{ctx}"));

            let (a_label, b_label) = git_labels(&raw);
            let want = filepair_body(&raw);
            let got = minigit::diff::unified::unified_bytes(OLD, NEW, &a_label, &b_label, ctx)
                .unwrap_or_else(|err| panic!("{class} -U{ctx}: unified_bytes failed: {err}"));
            assert!(
                !got.is_empty(),
                "{class} -U{ctx}: engine produced nothing for labels {a_label:?}/{b_label:?}"
            );
            assert_same_bytes(&format!("{class} -U{ctx}"), &want, &got);
            eprintln!("[coverage] engine class={class} U{ctx} labels={a_label:?}/{b_label:?} OK");
        }
    }
}

/// `-U0`/`-U3` 之外：空文件对非空文件（一侧没有内容）与二进制 + 含空格名字。
#[test]
fn engine_contentless_side_and_binary_line_follow_git() {
    // 一侧为空：git 仍按名字里的空格决定 TAB（与 /dev/null 的分支无关）。
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("empty"), b"").unwrap();
    write_rel(dir.path(), b"new file.txt", OLD);
    let raw = git_no_index(dir.path(), 3, b"empty", b"new file.txt");
    let (a_label, b_label) = git_labels(&raw);
    let got = minigit::diff::unified::unified_bytes(b"", OLD, &a_label, &b_label, 3)
        .expect("unified_bytes failed");
    assert_same_bytes("engine empty vs spaced name", &filepair_body(&raw), &got);

    // 二进制 + 含空格名字：`Binary files ... differ` 行**不补** TAB。
    let dir2 = tempfile::tempdir().expect("tempdir");
    write_rel(dir2.path(), b"bin old.dat", b"a\x00b\n");
    write_rel(dir2.path(), b"bin new.dat", b"a\x00c\n");
    let raw = git_no_index(dir2.path(), 3, b"bin old.dat", b"bin new.dat");
    let binary_line = raw
        .split_inclusive(|b| *b == b'\n')
        .find(|line| line.starts_with(b"Binary files "))
        .unwrap_or_else(|| panic!("no `Binary files` line in oracle: {}", esc(&raw)));
    assert!(
        !binary_line.contains(&b'\t'),
        "git pads the Binary line?! {}",
        esc(binary_line)
    );

    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    write_rel(repo.path(), b"bin old.dat", b"a\x00b\n");
    commit_all(repo.path(), "base binary");
    write_rel(repo.path(), b"bin new.dat", b"a\x00c\n");
    git_ok(repo.path(), &["add", "bin new.dat"]);
    let want = git_ok(repo.path(), &["diff", "--no-renames", "--staged", "--unified=3"]);
    let got = mg_run(repo.path(), &["diff", "--staged"]);
    assert!(got.status.success(), "mg diff --staged failed: {}", esc(&got.stderr));
    assert_same_bytes("staged binary with spaced names", &want, &got.stdout);
    assert!(
        !got
            .stdout
            .split_inclusive(|b| *b == b'\n')
            .find(|line| line.starts_with(b"Binary files "))
            .map(|line| line.contains(&b'\t'))
            .unwrap_or(false),
        "mg padded the Binary line: {}",
        esc(&got.stdout)
    );
}

// ---------------------------------------------------------------------------
// (B) CLI 级：真实仓库，三种调用组合 × {-U0, -U3}
// ---------------------------------------------------------------------------

#[test]
fn cli_name_matrix_matches_git_for_all_invocation_combos() {
    for (class, path) in wide_names() {
        let repo = tempfile::tempdir().expect("tempdir");
        let dir = repo.path();
        init_repo(dir);

        write_rel(dir, &path, OLD);
        commit_all(dir, "base");

        // (1) 工作区 vs index（默认 -U3 与 -U0）
        write_rel(dir, &path, NEW);
        compare_cli(
            dir,
            &["diff"],
            &["diff", "--no-renames"],
            &format!("{class}: worktree vs index -U3"),
        );
        compare_cli(
            dir,
            &["diff", "-U0"],
            &["diff", "--no-renames", "--unified=0"],
            &format!("{class}: worktree vs index -U0"),
        );

        // (2) index vs HEAD
        git_ok(dir, &["add", "-A"]);
        compare_cli(
            dir,
            &["diff", "--staged"],
            &["diff", "--no-renames", "--staged"],
            &format!("{class}: --staged"),
        );

        // (3) HEAD vs 工作区
        compare_cli(
            dir,
            &["diff", "HEAD"],
            &["diff", "--no-renames", "HEAD"],
            &format!("{class}: HEAD"),
        );
        eprintln!("[coverage] cli class={class} U3+U0+staged+HEAD OK");
    }
}

/// 新增 / 删除 / 二进制 三类边界，名字都含空格。
#[test]
fn cli_add_delete_binary_with_spaced_names_match_git() {
    // 新增含空格文件（`--- /dev/null` 不补，`+++ b/...` 补）。
    let repo = tempfile::tempdir().expect("tempdir");
    let dir = repo.path();
    init_repo(dir);
    fs::write(dir.join("seed.txt"), b"seed\n").unwrap();
    commit_all(dir, "base");
    write_rel(dir, b"new file.txt", OLD);
    git_ok(dir, &["add", "new file.txt"]);
    let want = git_ok(dir, &["diff", "--no-renames", "--staged"]);
    assert_eq!(git_labels(&want).0, "/dev/null");
    let got = mg_run(dir, &["diff", "--staged"]);
    assert!(got.status.success(), "mg failed: {}", esc(&got.stderr));
    assert_padding_rules(&want, "git", "staged new spaced file");
    assert_padding_rules(&got.stdout, "mg", "staged new spaced file");
    assert_same_bytes("staged new spaced file", &want, &got.stdout);

    // 删除含空格文件（`--- a/...` 补，`+++ /dev/null` 不补）。
    git_ok(dir, &["add", "-A"]);
    git_ok(
        dir,
        &[
            "-c",
            "user.name=V6b",
            "-c",
            "user.email=v6b@example.invalid",
            "commit",
            "-q",
            "-m",
            "add spaced file",
        ],
    );
    git_ok(dir, &["rm", "-q", "new file.txt"]);
    let want = git_ok(dir, &["diff", "--no-renames", "--staged"]);
    assert_eq!(git_labels(&want).1, "/dev/null");
    let got = mg_run(dir, &["diff", "--staged"]);
    assert!(got.status.success(), "mg failed: {}", esc(&got.stderr));
    assert_padding_rules(&want, "git", "staged deleted spaced file");
    assert_padding_rules(&got.stdout, "mg", "staged deleted spaced file");
    assert_same_bytes("staged deleted spaced file", &want, &got.stdout);

    // 二进制 + 含空格名字（工作区 vs index）。
    write_rel(dir, b"bin old.dat", b"a\x00b\n");
    git_ok(dir, &["add", "bin old.dat"]);
    git_ok(
        dir,
        &[
            "-c",
            "user.name=V6b",
            "-c",
            "user.email=v6b@example.invalid",
            "commit",
            "-q",
            "-m",
            "add binary",
        ],
    );
    write_rel(dir, b"bin old.dat", b"a\x00c\n");
    let want = git_ok(dir, &["diff", "--no-renames"]);
    let binary_line = want
        .split_inclusive(|b| *b == b'\n')
        .find(|line| line.starts_with(b"Binary files "))
        .unwrap_or_else(|| panic!("oracle has no Binary line: {}", esc(&want)));
    assert!(
        !binary_line.contains(&b'\t'),
        "git pads the Binary line?! {}",
        esc(binary_line)
    );
    let got = mg_run(dir, &["diff"]);
    assert!(got.status.success(), "mg failed: {}", esc(&got.stderr));
    assert_same_bytes("binary with space in name", &want, &got.stdout);
}

/// `diff --git` 行的 C-quote 规则没有被这次修复破坏：quoted 名字里**保留**的空格
/// 只影响 `--- `/`+++ `，不影响 `diff --git`（该行永远不补 TAB）。
#[test]
fn diff_git_header_stays_unpadded_for_quoted_spaced_names() {
    let repo = tempfile::tempdir().expect("tempdir");
    let dir = repo.path();
    init_repo(dir);
    write_rel(dir, b"qu\"ote sp.txt", OLD);
    commit_all(dir, "base");
    write_rel(dir, b"qu\"ote sp.txt", NEW);

    let want = git_ok(dir, &["diff", "--no-renames"]);
    let header = want.split_inclusive(|b| *b == b'\n').next().unwrap();
    assert!(
        header.contains(&b' '),
        "oracle header has no space, case not exercised: {}",
        esc(header)
    );
    assert!(
        !header.contains(&b'\t'),
        "`diff --git` line must never be padded: {}",
        esc(header)
    );
    let got = mg_run(dir, &["diff"]);
    assert!(got.status.success(), "mg failed: {}", esc(&got.stderr));
    assert_same_bytes("quoted spaced header (worktree)", &want, &got.stdout);

    git_ok(dir, &["add", "-A"]);
    let want = git_ok(dir, &["diff", "--no-renames", "--staged"]);
    let header = want.split_inclusive(|b| *b == b'\n').next().unwrap();
    assert!(!header.contains(&b'\t'), "`diff --git` line must never be padded");
    let got = mg_run(dir, &["diff", "--staged"]);
    assert!(got.status.success(), "mg failed: {}", esc(&got.stderr));
    assert_same_bytes("quoted spaced header (staged)", &want, &got.stdout);

    let head_out = mg_run(dir, &["diff", "HEAD"]);
    let git_head = base_git(dir, &["diff", "--no-renames", "HEAD"]);
    assert_same_bytes("quoted spaced header (HEAD)", &git_head.stdout, &head_out.stdout);
}

/// 已确认的能力边界（**不是** T6b 的回归）：`mg v1` 对非 UTF-8 路径直接报错，
/// 所以「非 UTF-8 名字」只能在引擎级覆盖，CLI 级只能在这里被记录成已知限制。
#[cfg(unix)]
#[test]
fn cli_non_utf8_path_is_a_known_v1_limitation() {
    let repo = tempfile::tempdir().expect("tempdir");
    let dir = repo.path();
    init_repo(dir);
    write_rel(dir, b"raw\xff sp.txt", OLD);
    commit_all(dir, "base");
    write_rel(dir, b"raw\xff sp.txt", NEW);
    let out = mg_run(dir, &["diff"]);
    assert!(
        !out.status.success(),
        "documented boundary changed: mg now handles non-UTF-8 paths; extend the CLI matrix"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("non-UTF8"),
        "unexpected error for non-UTF-8 path: {}",
        esc(&out.stderr)
    );
    // 引擎级仍然覆盖这一类（见 engine_filepair_lines_match_real_git_across_name_matrix）。
}
