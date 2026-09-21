//! V6 独立验证：T6（Myers diff 引擎 + unified 输出 + `mg diff`）。
//!
//! 真值 = **真实 git 的 stdout**：
//! * 引擎级：`git diff --no-index --no-renames -U<n> -- x y`（文件名固定成 `x`/`y`，
//!   所以头部正好是 `--- a/x` / `+++ b/y`，与 `diff_texts(.., "a/x", "b/y", ctx)`
//!   对齐；只比「从 `--- ` 行开始」的部分）。
//! * CLI 级：在 `tempfile` 里建真实 git 仓库，比 `git diff --no-renames ...` 的整体 stdout。
//!
//! 本文件**不硬编码任何 diff 文本**：所有期望值都在运行时由 `git` 生成。

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Instant;

// ---------------------------------------------------------------------------
// 真实 git 调用（配置只作用在子进程上，不 export 到本进程环境）
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

fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

fn esc(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).escape_debug().to_string()
}

/// 逐字节比较；失败时指出第一处不同的行，便于定位是 header 还是 body 的锅。
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

// ---------------------------------------------------------------------------
// 引擎级真值对拍
// ---------------------------------------------------------------------------

/// 丢掉 `diff --git` / `index` 行，返回「从 `--- ` 行开始」的字节。
fn strip_header(raw: &[u8]) -> Vec<u8> {
    let mut i = 0usize;
    while i < raw.len() {
        if raw[i..].starts_with(b"--- ") {
            return raw[i..].to_vec();
        }
        i = match raw[i..].iter().position(|&b| b == b'\n') {
            Some(offset) => i + offset + 1,
            None => break,
        };
    }
    Vec::new()
}

fn oracle_output(dir: &Path, old: &[u8], new: &[u8], ctx: usize) -> Vec<u8> {
    write_file(dir, "x", old);
    write_file(dir, "y", new);
    let out = base_git(
        dir,
        &[
            "diff",
            "--no-index",
            "--no-renames",
            &format!("-U{ctx}"),
            "--",
            "x",
            "y",
        ],
    );
    strip_header(&out.stdout)
}

fn engine_output(old: &[u8], new: &[u8], ctx: usize, label: &str) -> Vec<u8> {
    let text = minigit::diff::diff_texts(old, new, "a/x", "b/y", ctx)
        .unwrap_or_else(|err| panic!("diff_texts({label}, -U{ctx}) returned Err: {err}"));
    text.into_bytes()
}

fn compare_engine(dir: &Path, old: &[u8], new: &[u8], ctx: usize, label: &str) {
    let want = oracle_output(dir, old, new, ctx);
    let got = engine_output(old, new, ctx, label);
    assert_same_bytes(&format!("{label} (U{ctx})"), &want, &got);
}

fn bytes(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

fn numbered_lines(n: usize) -> String {
    let mut out = String::new();
    for i in 0..n {
        out.push_str(&format!("l{i}\n"));
    }
    out
}

fn matrix_cases() -> Vec<(String, Vec<u8>, Vec<u8>)> {
    let mut cases: Vec<(String, Vec<u8>, Vec<u8>)> = Vec::new();

    {
        let mut add = |name: &str, old: &str, new: &str| {
            cases.push((name.to_string(), bytes(old), bytes(new)));
        };
        add("identical", "a\nb\nc\n", "a\nb\nc\n");
        add("both_empty", "", "");
        add("add_from_empty", "", "a\nb\nc\n");
        add("delete_to_empty", "a\nb\nc\n", "");
        add("empty_vs_blank_line", "", "\n");
        add("blank_line_vs_empty", "\n", "");

        let head = "a\nb\nc\nd\ne\nf\ng\nh\n";
        add("insert_middle", head, "a\nb\nX\nc\nd\ne\nf\ng\nh\n");
        add("delete_middle", "a\nb\nX\nc\nd\ne\nf\ng\nh\n", head);
        add("replace_middle", "a\nb\nc\nd\ne\n", "a\nb\nCC\nd\ne\n");
        add("replace_first_line", "a\nb\nc\nd\ne\nf\ng\n", "A\nb\nc\nd\ne\nf\ng\n");
        add("replace_last_line", "a\nb\nc\nd\ne\nf\ng\n", "a\nb\nc\nd\ne\nf\nG\n");
        add("append_at_end", "a\nb\nc\n", "a\nb\nc\nd\n");
        add("prepend_at_start", "b\nc\nd\n", "a\nb\nc\nd\n");
        add("blank_line_added", "a\n\nb\n", "a\n\n\nb\n");

        add("no_trailing_newline_both", "a\nb\nc", "a\nb\nCC");
        add("no_trailing_newline_old_only", "a\nb\nc", "a\nb\nc\n");
        add("no_trailing_newline_new_only", "a\nb\nc\n", "a\nb\nc");

        add("crlf", "a\r\nb\r\nc\r\n", "a\r\nB\r\nc\r\n");
        add("isolated_cr", "a\rb\rc\r", "a\rB\rc\r");
        add("cjk", "中文\n第二行\n第三行\n", "中文\n第二行改了\n第三行\n");

        // 相同行滑动 + 缩进启发式（git 2.14+ 默认开启），失败时最能说明问题。
        add("block_swap", "a\nb\nc\nd\ne\nf\n", "c\nd\ne\nf\na\nb\n");
        add(
            "indent_heuristic",
            "fn main() {\n    let x = 1;\n    let y = 2;\n    foo();\n}\n",
            "fn main() {\n    let x = 1;\n    bar();\n    let y = 2;\n    foo();\n}\n",
        );
        // `def_ff` 边界：以字母/`_`/`$` 开头的行才是「函数行」，尾部空白要被裁掉。
        add(
            "func_suffix_edges",
            "fn beta() {\n   indented();\n\nlast_line  \n}\n",
            "fn beta() {\n   indented();\n\nLAST_LINE  \n}\n",
        );
        add(
            "func_suffix_trailing_space_tab",
            "alpha   \t\nb\nc\nd\ne\nf\ng\n",
            "alpha   \t\nb\nc\nd\ne\nf\nG\n",
        );
    }

    // 多 hunk（间隔远大于 2×context）。
    let mut distant_new = String::new();
    for i in 0..24 {
        if i == 2 || i == 18 {
            distant_new.push_str(&format!("changed {i}\n"));
        } else {
            distant_new.push_str(&format!("l{i}\n"));
        }
    }
    cases.push((
        "multi_hunk_distant".to_string(),
        numbered_lines(24).into_bytes(),
        distant_new.into_bytes(),
    ));

    // 全部行都不同。
    let mut all_new = String::new();
    for i in 0..12 {
        all_new.push_str(&format!("other {i}\n"));
    }
    cases.push((
        "all_lines_changed".to_string(),
        numbered_lines(12).into_bytes(),
        all_new.into_bytes(),
    ));
    cases
}

#[test]
fn engine_matrix_matches_git() {
    let dir = tempfile::tempdir().unwrap();
    for (name, old, new) in matrix_cases() {
        for ctx in [0usize, 1, 2, 3, 5] {
            compare_engine(dir.path(), &old, &new, ctx, &name);
        }
    }
}

#[test]
fn engine_long_single_line_matches_git() {
    let dir = tempfile::tempdir().unwrap();
    let mut old = "x".repeat(100_000);
    old.push('\n');
    let mut new = old.clone();
    new.replace_range(50_000..50_001, "Y");
    for ctx in [0usize, 3] {
        compare_engine(dir.path(), old.as_bytes(), new.as_bytes(), ctx, "long_single_line");
    }
}

/// 相邻 hunk 合并的边界：间隔恰好 2×context（应合并）与 2×context+1（不应合并）。
#[test]
fn engine_hunk_merge_boundary_matches_git() {
    let dir = tempfile::tempdir().unwrap();
    for ctx in [1usize, 2, 3] {
        for extra in [0usize, 1] {
            let gap = 2 * ctx + extra;
            let mut old = String::new();
            let mut new = String::new();
            for i in 0..(gap + 10) {
                let line = format!("line{i}\n");
                old.push_str(&line);
                if i == 2 || i == 3 + gap {
                    new.push_str(&format!("CHANGED{i}\n"));
                } else {
                    new.push_str(&line);
                }
            }
            let label = format!("merge_boundary ctx={ctx} gap={gap}");
            compare_engine(dir.path(), old.as_bytes(), new.as_bytes(), ctx, &label);

            // 顺带确认这个边界确实有区分度：git 在 2×ctx 时给 1 个 hunk、2×ctx+1 时给 2 个。
            let hunks = oracle_output(dir.path(), old.as_bytes(), new.as_bytes(), ctx)
                .split(|&b| b == b'\n')
                .filter(|line| line.starts_with(b"@@ "))
                .count();
            assert_eq!(
                hunks,
                if extra == 0 { 1 } else { 2 },
                "oracle hunk count for {label}"
            );
        }
    }
}

struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// 确定性随机 soak：小字母表（空行、缩进行、右括号、tab、CJK）最容易踩到
/// git 的滑动/缩进启发式。真值仍来自真实 git。
#[test]
fn engine_randomized_soak_matches_git() {
    let dir = tempfile::tempdir().unwrap();
    const ALPHABET: [&str; 9] = [
        "a\n",
        "b\n",
        "\n",
        "  x\n",
        "\ty\n",
        "   }\n",
        "}\n",
        "中\n",
        "long_line_with_no_spaces\n",
    ];
    let mut rng = Lcg::new(0x5eed_1234_abcd_ef01);
    for case in 0..250 {
        let n = rng.below(26) as usize;
        let m = rng.below(26) as usize;
        let mut old = Vec::new();
        let mut new = Vec::new();
        for _ in 0..n {
            old.extend_from_slice(ALPHABET[rng.below(9) as usize].as_bytes());
        }
        for _ in 0..m {
            new.extend_from_slice(ALPHABET[rng.below(9) as usize].as_bytes());
        }
        if rng.below(3) == 0 {
            if old.last() == Some(&b'\n') {
                old.pop();
            }
            if new.last() == Some(&b'\n') {
                new.pop();
            }
        }
        let ctx = rng.below(4) as usize;
        compare_engine(dir.path(), &old, &new, ctx, &format!("soak#{case}"));
    }
}

/// 引擎契约：编辑脚本按行序完整覆盖两侧，`Equal` 段两侧内容必须一致。
#[test]
fn myers_script_covers_both_sides() {
    use minigit::diff::{myers, split_lines, Op};

    let mut inputs: Vec<(Vec<u8>, Vec<u8>)> = matrix_cases()
        .into_iter()
        .map(|(_, old, new)| (old, new))
        .collect();
    let mut rng = Lcg::new(0x0bad_c0de_dead_beef);
    const ALPHABET: [&str; 6] = ["a\n", "b\n", "\n", "  x\n", "}\n", "中\n"];
    for _ in 0..200 {
        let mut old = Vec::new();
        let mut new = Vec::new();
        for _ in 0..rng.below(20) {
            old.extend_from_slice(ALPHABET[rng.below(6) as usize].as_bytes());
        }
        for _ in 0..rng.below(20) {
            new.extend_from_slice(ALPHABET[rng.below(6) as usize].as_bytes());
        }
        inputs.push((old, new));
    }

    for (case, (old, new)) in inputs.iter().enumerate() {
        let old_lines = split_lines(old);
        let new_lines = split_lines(new);
        let edits = myers(old, new).unwrap_or_else(|err| panic!("myers case#{case} err: {err}"));
        let mut oi = 0usize;
        let mut ni = 0usize;
        for edit in &edits {
            assert_eq!(edit.old.start, oi, "case#{case}: old gap at {edit:?}");
            assert_eq!(edit.new.start, ni, "case#{case}: new gap at {edit:?}");
            match edit.op {
                Op::Equal => {
                    assert!(!edit.old.is_empty(), "case#{case}: empty Equal");
                    assert_eq!(edit.old.len(), edit.new.len(), "case#{case}: Equal length");
                    for k in 0..edit.old.len() {
                        assert_eq!(
                            old_lines[oi + k], new_lines[ni + k],
                            "case#{case}: Equal content differs at {k}"
                        );
                    }
                }
                Op::Delete => {
                    assert!(edit.new.is_empty(), "case#{case}: Delete touched new");
                    assert!(!edit.old.is_empty(), "case#{case}: empty Delete");
                }
                Op::Insert => {
                    assert!(edit.old.is_empty(), "case#{case}: Insert touched old");
                    assert!(!edit.new.is_empty(), "case#{case}: empty Insert");
                }
            }
            oi = edit.old.end;
            ni = edit.new.end;
        }
        assert_eq!(oi, old_lines.len(), "case#{case}: old not fully covered");
        assert_eq!(ni, new_lines.len(), "case#{case}: new not fully covered");
    }
}

/// B4 反例：5000 行里改 1 行必须秒级返回且结果正确（不 panic）。
#[test]
fn engine_large_single_change_is_fast_and_correct() {
    let dir = tempfile::tempdir().unwrap();
    let old = numbered_lines(5000);
    let new = old.replacen("l2500\n", "l2500 CHANGED\n", 1);
    assert_ne!(old, new);

    let start = Instant::now();
    let got = engine_output(old.as_bytes(), new.as_bytes(), 3, "large_single_change");
    let elapsed = start.elapsed();
    let want = oracle_output(dir.path(), old.as_bytes(), new.as_bytes(), 3);
    assert_same_bytes("large_single_change", &want, &got);
    assert!(elapsed.as_secs() < 5, "engine took {elapsed:?} for 5000 lines");
}

/// 规模保护反例：20000 行 × 20000 行「完全不同」不得卡死/panic。
#[test]
fn engine_large_disjoint_input_returns_in_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut old = String::new();
    let mut new = String::new();
    for i in 0..20_000 {
        old.push_str(&format!("old{i}\n"));
        new.push_str(&format!("new{i}\n"));
    }
    let start = Instant::now();
    let got = engine_output(old.as_bytes(), new.as_bytes(), 3, "large_disjoint");
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs() < 60, "engine took {elapsed:?} for 20000x20000");

    let want = oracle_output(dir.path(), old.as_bytes(), new.as_bytes(), 3);
    assert_eq!(want.len(), got.len(), "large_disjoint length");
    assert_eq!(want, got, "large_disjoint bytes");
}

// ---------------------------------------------------------------------------
// CLI 端到端（真实仓库里的 `mg diff` vs `git diff`）
// ---------------------------------------------------------------------------

fn run_cli(dir: &Path, args: &[&str]) -> Output {
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

fn assert_cli_matches_git(dir: &Path, mg_args: &[&str], git_args: &[&str], label: &str) {
    let _ = cli_matches_git(dir, mg_args, git_args, label);
}

/// 返回 git（真值）的 stdout，便于调用方判断这次对拍是否非空。
fn cli_matches_git(dir: &Path, mg_args: &[&str], git_args: &[&str], label: &str) -> Vec<u8> {
    let mg = run_cli(dir, mg_args);
    assert!(
        mg.status.success(),
        "{label}: mg {mg_args:?} exited {:?}; stderr={}",
        mg.status,
        esc(&mg.stderr)
    );
    let git = base_git(dir, git_args);
    assert_same_bytes(label, &git.stdout, &mg.stdout);
    git.stdout
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack.windows(needle.len()).any(|window| window == needle)
}

/// 非空真值里必须出现的标记（防止对拍退化成「空 == 空」）。
fn assert_contains(haystack: &[u8], needle: &str, label: &str) {
    assert!(
        contains_bytes(haystack, needle.as_bytes()),
        "{label}: oracle output is missing {needle:?}; output was:\n{}",
        esc(haystack)
    );
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
            "user.name=V6",
            "-c",
            "user.email=v6@example.invalid",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

/// 真实仓库：已提交基线 + 未跟踪/未暂存/已暂存/删除/二进制/CJK 路径/无尾换行。
fn build_fixture() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    init_repo(dir);
    write_file(dir, "f.txt", b"one\ntwo\nthree\nfour\nfive\nsix\n");
    write_file(dir, "g.txt", b"alpha\nbeta\ngamma\ndelta\n");
    write_file(dir, "gone.txt", b"delete me\nline 2\n");
    write_file(dir, "bin.dat", b"\x00\x01\x02binary v1\n");
    write_file(dir, "noeol.txt", b"keep no newline");
    write_file(dir, "中文.txt", "中文内容\n第二行\n".as_bytes());
    write_file(dir, "sub/deep.txt", b"deep\n");
    commit_all(dir, "base");

    // 未暂存改动。
    write_file(dir, "f.txt", b"one\ntwo\nTHREE\nfour\nfive\nsix\n");
    fs::remove_file(dir.join("gone.txt")).unwrap();
    write_file(dir, "bin.dat", b"\x00\x01\x02binary v2 changed\n");
    write_file(dir, "noeol.txt", b"keep no newline CHANGED");
    write_file(dir, "中文.txt", "中文内容\n第二行改了\n".as_bytes());
    write_file(dir, "untracked.txt", b"untracked must not appear\n");

    // 已暂存改动。
    write_file(dir, "g.txt", b"alpha\nBETA\ngamma\ndelta\n");
    write_file(dir, "added.txt", b"brand new staged file\nsecond line\n");
    git_ok(dir, &["add", "g.txt", "added.txt"]);
    repo
}

#[test]
fn cli_worktree_vs_index_matches_git() {
    let repo = build_fixture();
    let dir = repo.path();
    let oracle = base_git(dir, &["diff", "--no-renames"]).stdout;
    for marker in [
        "diff --git a/f.txt b/f.txt",
        "diff --git a/gone.txt b/gone.txt",
        "deleted file mode 100644",
        "diff --git a/bin.dat b/bin.dat",
        "Binary files a/bin.dat and b/bin.dat differ",
        "No newline at end of file",
        "diff --git \"a/\\344\\270\\255\\346\\226\\207.txt\"",
    ] {
        assert_contains(&oracle, marker, "worktree-vs-index oracle");
    }
    assert!(
        !contains_bytes(&oracle, b"untracked.txt"),
        "untracked file must not appear in `git diff`"
    );

    assert_cli_matches_git(dir, &["diff"], &["diff", "--no-renames"], "mg diff");
    assert_cli_matches_git(
        dir,
        &["diff", "-U0"],
        &["diff", "--no-renames", "-U0"],
        "mg diff -U0",
    );
    assert_cli_matches_git(
        dir,
        &["diff", "--", "f.txt"],
        &["diff", "--no-renames", "--", "f.txt"],
        "mg diff -- f.txt",
    );
    assert_cli_matches_git(
        dir,
        &["diff", "--", "sub"],
        &["diff", "--no-renames", "--", "sub"],
        "mg diff -- sub",
    );
    assert_cli_matches_git(
        dir,
        &["diff", "--", "nope.txt"],
        &["diff", "--no-renames", "--", "nope.txt"],
        "mg diff -- nope.txt",
    );
}

#[test]
fn cli_staged_matches_git() {
    let repo = build_fixture();
    let dir = repo.path();
    let oracle = base_git(dir, &["diff", "--no-renames", "--staged"]).stdout;
    for marker in [
        "diff --git a/g.txt b/g.txt",
        "diff --git a/added.txt b/added.txt",
        "new file mode 100644",
    ] {
        assert_contains(&oracle, marker, "staged oracle");
    }
    assert_cli_matches_git(
        dir,
        &["diff", "--staged"],
        &["diff", "--no-renames", "--staged"],
        "mg diff --staged",
    );
    assert_cli_matches_git(
        dir,
        &["diff", "--staged", "-U0"],
        &["diff", "--no-renames", "--staged", "-U0"],
        "mg diff --staged -U0",
    );
}

#[test]
fn cli_rev_matches_git() {
    let repo = build_fixture();
    let dir = repo.path();
    assert_cli_matches_git(
        dir,
        &["diff", "HEAD"],
        &["diff", "--no-renames", "HEAD"],
        "mg diff HEAD",
    );
    assert_cli_matches_git(
        dir,
        &["diff", "--staged", "HEAD"],
        &["diff", "--no-renames", "--staged", "HEAD"],
        "mg diff --staged HEAD",
    );
    let oid = String::from_utf8(git_ok(dir, &["rev-parse", "HEAD"])).unwrap();
    let oid = oid.trim().to_string();
    assert_cli_matches_git(
        dir,
        &["diff", &oid],
        &["diff", "--no-renames", &oid],
        "mg diff <oid>",
    );
}

#[test]
fn cli_clean_repo_prints_nothing() {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    init_repo(dir);
    write_file(dir, "a.txt", b"stable\n");
    commit_all(dir, "base");

    assert_cli_matches_git(dir, &["diff"], &["diff", "--no-renames"], "clean mg diff");
    assert_cli_matches_git(
        dir,
        &["diff", "--staged"],
        &["diff", "--no-renames", "--staged"],
        "clean mg diff --staged",
    );
}

#[test]
fn cli_binary_is_reported_as_binary_and_matches_git() {
    let repo = build_fixture();
    let dir = repo.path();
    let out = run_cli(dir, &["diff"]);
    assert!(out.status.success(), "mg diff failed: {}", esc(&out.stderr));

    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Binary files a/bin.dat and b/bin.dat differ"),
        "expected binary marker, got:\n{text}"
    );
    assert!(
        !out.stdout.contains(&0u8),
        "raw NUL byte leaked into `mg diff` stdout"
    );
    let git = base_git(dir, &["diff", "--no-renames"]);
    assert_same_bytes("binary mg diff", &git.stdout, &out.stdout);
}

#[test]
fn cli_mode_change_matches_git() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path();
        init_repo(dir);
        write_file(dir, "script.sh", b"#!/bin/sh\necho hi\n");
        write_file(dir, "both.sh", b"#!/bin/sh\necho both\n");
        commit_all(dir, "base");

        for name in ["script.sh", "both.sh"] {
            let path = dir.join(name);
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        // 一个只改 mode，另一个 mode + 内容都改。
        write_file(dir, "both.sh", b"#!/bin/sh\necho both CHANGED\n");
        assert_cli_matches_git(
            dir,
            &["diff"],
            &["diff", "--no-renames"],
            "mode-only + mode+content",
        );

        git_ok(dir, &["add", "-A"]);
        assert_cli_matches_git(
            dir,
            &["diff", "--staged"],
            &["diff", "--no-renames", "--staged"],
            "staged mode change",
        );
    }
}

// ---------------------------------------------------------------------------
// 随机化 CLI soak：真实仓库 + 随机改动，逐字节对比 `mg diff` / `git diff`
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Crlf,
    Binary,
}

const POOL: [(&str, Kind); 6] = [
    ("f1.txt", Kind::Text),
    ("f2.txt", Kind::Text),
    ("sub/f3.txt", Kind::Text),
    ("crlf.txt", Kind::Crlf),
    ("bin.dat", Kind::Binary),
    ("中文.txt", Kind::Text),
];

fn random_text(rng: &mut Lcg, max_lines: u64, crlf: bool) -> Vec<u8> {
    const ALPHA: [&str; 7] = ["a\n", "b\n", "  indented\n", "\n", "}\n", "中文\n", "\t\n"];
    let mut out = Vec::new();
    for _ in 0..rng.below(max_lines) {
        let line = ALPHA[rng.below(7) as usize];
        if crlf {
            out.extend_from_slice(line.trim_end_matches('\n').as_bytes());
            out.extend_from_slice(b"\r\n");
        } else {
            out.extend_from_slice(line.as_bytes());
        }
    }
    if rng.below(4) == 0 {
        if out.last() == Some(&b'\n') {
            out.pop();
        }
        if out.last() == Some(&b'\r') {
            out.pop();
        }
    }
    out
}

fn random_binary(rng: &mut Lcg) -> Vec<u8> {
    let mut out = vec![0x00, 0x01, 0x02, 0xff];
    out.extend_from_slice(format!("binary payload {}\n", rng.below(10_000)).as_bytes());
    out.push(0x00);
    out
}

fn random_content(kind: Kind, rng: &mut Lcg) -> Vec<u8> {
    match kind {
        Kind::Text => random_text(rng, 14, false),
        Kind::Crlf => random_text(rng, 14, true),
        Kind::Binary => random_binary(rng),
    }
}

#[test]
fn cli_randomized_repo_soak_matches_git() {
    let mut rng = Lcg::new(0xfeed_face_1234_5678);
    let mut nonempty = 0usize;
    let mut total = 0usize;

    for case in 0..30 {
        let repo = tempfile::tempdir().unwrap();
        let dir = repo.path();
        init_repo(dir);
        for (name, kind) in POOL {
            write_file(dir, name, &random_content(kind, &mut rng));
        }
        commit_all(dir, "base");

        for (name, kind) in POOL {
            match rng.below(4) {
                0 => write_file(dir, name, &random_content(kind, &mut rng)),
                1 => {
                    let _ = fs::remove_file(dir.join(name));
                }
                _ => {}
            }
        }
        write_file(dir, "new_untracked.txt", &random_content(Kind::Text, &mut rng));

        for (name, _) in POOL {
            if rng.below(2) == 0 {
                git_ok(dir, &["add", "-A", "--", name]);
            }
        }
        if rng.below(2) == 0 {
            git_ok(dir, &["add", "-A", "--", "new_untracked.txt"]);
        }

        let ctx = format!("-U{}", rng.below(4));
        let oracle = cli_matches_git(
            dir,
            &["diff", ctx.as_str()],
            &["diff", "--no-renames", ctx.as_str()],
            &format!("soak#{case} worktree"),
        );
        if !oracle.is_empty() {
            nonempty += 1;
        }
        total += 1;

        let oracle = cli_matches_git(
            dir,
            &["diff", "--staged", ctx.as_str()],
            &["diff", "--no-renames", "--staged", ctx.as_str()],
            &format!("soak#{case} staged"),
        );
        if !oracle.is_empty() {
            nonempty += 1;
        }
        total += 1;
    }

    eprintln!("cli soak: {nonempty}/{total} comparisons had a non-empty oracle");
    assert!(
        nonempty * 3 >= total,
        "soak was mostly vacuous: only {nonempty}/{total} comparisons had a non-empty oracle"
    );
}

// ---------------------------------------------------------------------------
// 已知限制的边界确认（controller 备注：rename 差异不算 FAIL）
// ---------------------------------------------------------------------------

/// V6 发现的真 FAIL（故意 `#[ignore]`，避免把共享 checkout 的测试套件弄红；
/// 完整证据见 `.orch/waves/W2/T6-diff/verify-scratch/known-fail-space-in-path.txt`）。
///
/// 路径含空格时，真实 git 会在 `--- `/`+++ ` 的名字后补一个 TAB
/// （`--- a/a b.txt\t`，quoted 名字也一样），`mg` 不补 → 逐字节不一致。
#[test]
#[ignore = "V6 FAIL: git appends a TAB to ---/+++ lines for paths containing a space; mg does not"]
fn cli_space_in_path_tab_padding() {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    init_repo(dir);
    write_file(dir, "a b.txt", b"line1\nline2\n");
    commit_all(dir, "base");
    write_file(dir, "a b.txt", b"line1\nCHANGED\n");
    assert_cli_matches_git(
        dir,
        &["diff"],
        &["diff", "--no-renames"],
        "space in path (known FAIL)",
    );
}

#[test]
fn cli_rename_limitation_is_only_renames() {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    init_repo(dir);
    let mut content = String::new();
    for i in 0..12 {
        content.push_str(&format!("rename line {i}\n"));
    }
    write_file(dir, "old_name.txt", content.as_bytes());
    commit_all(dir, "base");
    git_ok(dir, &["mv", "old_name.txt", "new_name.txt"]);

    // v1 不做 rename/copy 检测：mg 必须与 `git diff --no-renames` 逐字节一致。
    assert_cli_matches_git(
        dir,
        &["diff", "--staged"],
        &["diff", "--no-renames", "--staged"],
        "rename vs --no-renames",
    );

    // 打开 rename 检测后 git 才多出 rename 行；mg 不得输出这些行。
    let with_renames = base_git(dir, &["-c", "diff.renames=true", "diff", "--staged"]).stdout;
    let mg_out = run_cli(dir, &["diff", "--staged"]).stdout;
    assert_ne!(
        with_renames, mg_out,
        "git rename detection should have changed the output"
    );
    assert_contains(&with_renames, "rename from old_name.txt", "rename oracle");
    assert_contains(&with_renames, "rename to new_name.txt", "rename oracle");
    assert!(!contains_bytes(&mg_out, b"rename from"), "mg must not emit rename lines");
}

#[test]
fn cli_non_utf8_text_matches_git() {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    init_repo(dir);
    // Latin-1 的 'é'（0xe9），没有 NUL -> git 当普通文本处理。
    let old = b"caf\xe9 au lait\nsecond\n".to_vec();
    let new = b"caf\xe9 au lait\nsecond CHANGED\n".to_vec();
    write_file(dir, "latin1.txt", &old);
    commit_all(dir, "base");
    write_file(dir, "latin1.txt", &new);

    let out = run_cli(dir, &["diff"]);
    assert!(out.status.success(), "mg diff failed: {}", esc(&out.stderr));
    assert!(!out.stdout.contains(&0u8), "no NUL bytes expected");

    let git = base_git(dir, &["diff", "--no-renames"]);
    assert!(
        contains_bytes(&git.stdout, b"caf\xe9 au lait"),
        "latin-1 oracle lost the raw 0xe9 byte:\n{}",
        esc(&git.stdout)
    );
    assert_same_bytes("non-utf8 text", &git.stdout, &out.stdout);
}
