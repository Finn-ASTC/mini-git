//! unified diff 文本生成。**T6（omp）实现范围**。
//!
//! 输出格式（与 `git diff` 对齐）：
//!
//! ```text
//! diff --git a/f b/f
//! index <old7>..<new7> 100644
//! --- a/f
//! +++ b/f
//! @@ -1,3 +1,4 @@
//!  context
//! -removed
//! +added
//! \ No newline at end of file
//! ```
//!
//! 细节（都是差分测试会踩的坑）：
//! * 无改动时返回空字符串（调用方据此决定是否输出 header）。
//! * hunk 头：长度为 1 时省略 `,1`；长度为 0 时起点写成「前一行」。
//! * 两个 hunk 距离 ≤ 2×context 时合并成一个。
//! * 输入末尾无 `\n` 时，在该行后输出 `\ No newline at end of file`。
//!
//! # 与 git 逐字节对齐的额外规则（`xdiff/xemit.c`）
//!
//! * `@@ -a,b +c,d @@` 之后 git 会再打印**函数名后缀**：从 hunk 起点往前找第一行
//!   以 `[A-Za-z_$]` 开头的行（`def_ff`），去掉尾部空白、最长 80 字节，格式是
//!   ` @@ <那个行>`。找不到就不打印后缀（第一行的 hunk、或被空白/缩进行挡住时）。
//!   这条在 `md`/纯文本文件里同样生效，所以不实现它就无法与 git 逐字节一致。
//! * `--- ` / `+++ ` 行：名字（含 `a/` `b/` 前缀、可能已 C-quoted，或就是 `/dev/null`）里
//!   出现空格字节时，行尾补一个 TAB（`strchr(line, ' ') ? "\t" : ""`，见 `diff.c` 的
//!   `DIFF_SYMBOL_FILEPAIR_MINUS` / `DIFF_SYMBOL_FILEPAIR_PLUS`）。`diff --git` 行与
//!   `Binary files ... differ` 行是另外两条 emit 路径，**从不**补。
//! * context 行取自**新文件**（git 的 `xdl_emit_diff` 一律从 `xdf2` 取）；
//!   未变更区间两侧内容相同，所以结果一致。
//! * `Hunk::old_start` / `new_start` 是**从 1 开始**的行号（空区间就是插入位置
//!   对应的 1-based 行号，例如新文件是 `old_start == 1`），打印时长度为 0 才减 1。
//!
//! `hunks()` 承载不了函数名（`Hunk` 没有对应字段、且 `Hunk` 只依赖编辑脚本），
//! 所以函数名后缀由 `unified()` 自己按同样的规则从 `old` 里现算。

use crate::error::{Error, Result};

use super::myers::{self, Edit};
use super::split_lines;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    Context(Vec<u8>),
    Removed(Vec<u8>),
    Added(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// 旧文件侧的 1-based 起始行号。空区间（`old_len == 0`）时是**插入位置**
    /// 对应的行号，例如新文件是 1（打印成 `-0,0`）。
    pub old_start: usize,
    pub old_len: usize,
    /// 新文件侧的 1-based 起始行号（空区间同理）。
    pub new_start: usize,
    pub new_len: usize,
    /// hunk 正文，顺序与 git 一致：context 来自新文件，删除行在前、添加行在后。
    /// 每行都带着自己的换行（末行没有换行时就没有），够调用方判断要不要打
    /// `\ No newline at end of file`。
    pub lines: Vec<HunkLine>,
}

/// 把编辑脚本按 context 归并成 hunk。
///
/// 归并规则与 `xdl_get_hunk` 一致：相邻变更块在旧文件里的间隔 ≤ `2 * context`
/// 就并进同一个 hunk。
pub fn hunks(edits: &[Edit], old: &[u8], new: &[u8], context: usize) -> Result<Vec<Hunk>> {
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let atoms = myers::change_atoms(edits);
    if atoms.is_empty() {
        return Ok(Vec::new());
    }

    let max_common = 2 * context;
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < atoms.len() {
        let first = idx;
        let mut last = idx;
        while last + 1 < atoms.len() {
            // `distance = 下一个变更块的旧起点 - 当前变更块的旧终点`
            let distance = atoms[last + 1].0.start - atoms[last].0.end;
            if distance > max_common {
                break;
            }
            last += 1;
        }

        let first_atom = &atoms[first];
        let last_atom = &atoms[last];

        // `s1 = max(i1 - ctx, 0)`、`lctx = min(ctx, 旧文件剩余, 新文件剩余)`。
        let s1 = first_atom.0.start.saturating_sub(context);
        let s2 = first_atom.1.start.saturating_sub(context);
        let lctx = context
            .min(old_lines.len() - last_atom.0.end)
            .min(new_lines.len() - last_atom.1.end);
        let e1 = last_atom.0.end + lctx;
        let e2 = last_atom.1.end + lctx;

        let mut lines = Vec::new();
        // 前置上下文：git 只按新文件的下标走（`for (; s2 < xch->i2; s2++)`）。
        let mut cursor = s2;
        while cursor < first_atom.1.start {
            lines.push(HunkLine::Context(new_lines[cursor].to_vec()));
            cursor += 1;
        }

        for (position, atom) in atoms[first..=last].iter().enumerate() {
            if position > 0 {
                let previous = &atoms[first + position - 1];
                let mut o = previous.0.end;
                let mut n = previous.1.end;
                while o < atom.0.start && n < atom.1.start {
                    lines.push(HunkLine::Context(new_lines[n].to_vec()));
                    o += 1;
                    n += 1;
                }
            }
            for line in &old_lines[atom.0.clone()] {
                lines.push(HunkLine::Removed(line.to_vec()));
            }
            for line in &new_lines[atom.1.clone()] {
                lines.push(HunkLine::Added(line.to_vec()));
            }
        }

        // 后置上下文：同样按新文件下标（`for (s2 = xche->i2 + chg2; s2 < e2; s2++)`）。
        let mut cursor = last_atom.1.end;
        while cursor < e2 {
            lines.push(HunkLine::Context(new_lines[cursor].to_vec()));
            cursor += 1;
        }

        out.push(Hunk {
            old_start: s1 + 1,
            old_len: e1 - s1,
            new_start: s2 + 1,
            new_len: e2 - s2,
            lines,
        });

        idx = last + 1;
    }

    Ok(out)
}

pub fn unified(
    old: &[u8],
    new: &[u8],
    a_label: &str,
    b_label: &str,
    context: usize,
) -> Result<String> {
    String::from_utf8(unified_bytes(old, new, a_label, b_label, context)?).map_err(|_| {
        Error::Other(
            "unified diff of non-UTF-8 content cannot be represented as a String \
             (use `diff::unified::unified_bytes` for raw bytes)"
                .to_string(),
        )
    })
}

/// 同 `unified`，但返回原始字节：`mg diff` 必须能原样输出非 UTF-8 内容
/// （真实 git 不做编码转换）。`unified` 是这个函数的 UTF-8 视图。
pub fn unified_bytes(
    old: &[u8],
    new: &[u8],
    a_label: &str,
    b_label: &str,
    context: usize,
) -> Result<Vec<u8>> {
    let edits = myers::myers(old, new)?;
    let hunks = hunks(&edits, old, new, context)?;
    if hunks.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    push_prefixed_line(&mut out, b"--- ", a_label.as_bytes());
    push_prefixed_line(&mut out, b"+++ ", b_label.as_bytes());

    let old_lines = split_lines(old);

    // `funclineprev`：上一个 hunk 向后扫描的**排他**下界（首个 hunk 是 -1）。
    let mut funclineprev = -1i64;
    let mut func: Vec<u8> = Vec::new();
    for hunk in &hunks {
        let s1 = hunk.old_start as i64 - 1;
        if let Some(found) = func_line(&old_lines, s1 - 1, funclineprev) {
            func = found;
        }
        funclineprev = s1 - 1;

        push_hunk_header(&mut out, hunk, &func);
        for line in &hunk.lines {
            let (prefix, bytes) = match line {
                HunkLine::Context(bytes) => (b' ', bytes),
                HunkLine::Removed(bytes) => (b'-', bytes),
                HunkLine::Added(bytes) => (b'+', bytes),
            };
            out.push(prefix);
            out.extend_from_slice(bytes);
            if bytes.last() != Some(&b'\n') {
                out.extend_from_slice(b"\n\\ No newline at end of file\n");
            }
        }
    }

    Ok(out)
}

/// `--- ` / `+++ ` 行。
///
/// git 对这两行做的是同一件事（`diff.c` 的 `DIFF_SYMBOL_FILEPAIR_MINUS` /
/// `DIFF_SYMBOL_FILEPAIR_PLUS`）：
///
/// ```c
/// fprintf(o->file, "%s%s--- %s%s%s\n", ..., line, reset, strchr(line, ' ') ? "\t" : "");
/// ```
///
/// 即：**这个名字**（已含 `a/` `b/` 前缀、可能已 C-quoted，或就是 `/dev/null`）
/// 里只要出现空格字节 `0x20`，行尾就补一个 TAB。unified diff 的传统格式里名字后面
/// 跟的是「时间戳」字段，补 TAB 是为了让补丁解析器仍能切出名字。
///
/// 三点必须注意：
/// * 判定**逐 label**：`--- /dev/null` 一侧永远不补，`+++ b/new file.txt` 一侧补。
/// * 只看 label 字节，不看前缀（`--- ` 自己带的那个空格不算）。TAB、CJK、非 UTF-8
///   这些会被 `quote_path` 转义成 `\t` / `\ooo` 的字节，转义后不含空格 → 不补；
///   quoted 形式里保留下来的空格（如 `"a/q\"uo te.txt"`）算 → 补。
/// * `diff --git` 行与 `Binary files ... differ` 行不补（另两条 emit 路径）。
///
/// 这里的 text 不含 NUL（与 git 的 `strchr` 语义一致）：路径里的 NUL 会被
/// `quote_path` 转义成 `\000`。
fn push_prefixed_line(out: &mut Vec<u8>, prefix: &[u8], text: &[u8]) {
    out.extend_from_slice(prefix);
    out.extend_from_slice(text);
    if text.contains(&b' ') {
        out.push(b'\t');
    }
    out.push(b'\n');
}

fn push_num(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(value.to_string().as_bytes());
}

/// `xdl_format_hunk_hdr`：长度为 1 时省略 `,1`，长度为 0 时起点减 1。
fn push_hunk_header(out: &mut Vec<u8>, hunk: &Hunk, func: &[u8]) {
    out.extend_from_slice(b"@@ -");
    push_num(
        out,
        if hunk.old_len != 0 {
            hunk.old_start
        } else {
            hunk.old_start - 1
        },
    );
    if hunk.old_len != 1 {
        out.push(b',');
        push_num(out, hunk.old_len);
    }
    out.extend_from_slice(b" +");
    push_num(
        out,
        if hunk.new_len != 0 {
            hunk.new_start
        } else {
            hunk.new_start - 1
        },
    );
    if hunk.new_len != 1 {
        out.push(b',');
        push_num(out, hunk.new_len);
    }
    out.extend_from_slice(b" @@");
    if !func.is_empty() {
        out.push(b' ');
        out.extend_from_slice(func);
    }
    out.push(b'\n');
}

/// `def_ff`：以 `[A-Za-z_$]` 开头的行才算「函数行」，去掉尾部空白、截断到 80 字节。
fn def_ff(line: &[u8]) -> Option<Vec<u8>> {
    const FUNC_BUF: usize = 80;
    if line.is_empty() {
        return None;
    }
    let first = line[0];
    if !(first.is_ascii_alphabetic() || first == b'_' || first == b'$') {
        return None;
    }
    let mut len = line.len().min(FUNC_BUF);
    while len > 0 && is_space(line[len - 1]) {
        len -= 1;
    }
    Some(line[..len].to_vec())
}

/// `get_func_line`：从 `start` 出发，向 `limit`（排他）方向扫描第一行「函数行」。
fn func_line(lines: &[&[u8]], start: i64, limit: i64) -> Option<Vec<u8>> {
    let nrec = lines.len() as i64;
    let step = if start > limit { -1 } else { 1 };
    let mut l = start;
    while l != limit && (0..nrec).contains(&l) {
        if let Some(found) = def_ff(lines[l as usize]) {
            return Some(found);
        }
        l += step;
    }
    None
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// 跑真实 git 的 `diff --no-index`，返回**从 `--- ` 行开始**的输出。
    ///
    /// `git diff --no-index` 会把命令行上的真实路径写进头部（`--- a/x`），
    /// 所以只要文件名固定成 `x` / `y`，头部就与 `unified(.., "a/x", "b/y", ..)`
    /// 完全对齐；这里把 `diff --git` / `index` 两行也一并丢掉，只比自己生成的
    /// 那部分。真实 git 是本项目的唯一真值（见 `ORCHESTRATION.md`）。
    fn git_body(dir: &Path, context: usize, a: &str, b: &str) -> Vec<u8> {
        from_filepair(&git_no_index(dir, context, OsStr::new(a), OsStr::new(b)))
    }

    /// 原始字节 → `&OsStr`（非 UTF-8 的名字只在 unix 上存在）。
    #[cfg(unix)]
    fn os_str(bytes: &[u8]) -> &OsStr {
        use std::os::unix::ffi::OsStrExt;
        OsStr::from_bytes(bytes)
    }

    #[cfg(not(unix))]
    fn os_str(bytes: &[u8]) -> &OsStr {
        OsStr::new(std::str::from_utf8(bytes).expect("non-UTF-8 names are unix-only"))
    }

    /// `git diff --no-index` 的**完整** stdout（含 `diff --git` / `index` 行）。
    /// 名字按原始字节给，所以可以造非 UTF-8 / 带 TAB 的名字。
    fn git_no_index(dir: &Path, context: usize, a: &OsStr, b: &OsStr) -> Vec<u8> {
        let out = Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .args([
                "-c",
                "core.autocrlf=false",
                "diff",
                "--no-index",
                &format!("--unified={context}"),
                "--",
            ])
            .arg(a)
            .arg(b)
            .output()
            .expect("failed to spawn git");
        // --no-index 的退出码 1 = 「有差异」，不是错误。
        out.stdout
    }

    /// 在 `dir` 里跑 git（`init` / `add` / `diff` 这些），返回 stdout；失败直接 panic。
    fn git_repo(dir: &Path, args: &[&str]) -> Vec<u8> {
        let out = Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .args(args)
            .output()
            .expect("failed to spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    /// 丢掉 `diff --git` / `index` 行，返回「从 `--- ` 行开始」的字节。
    fn from_filepair(raw: &[u8]) -> Vec<u8> {
        let mut offset = 0usize;
        for line in raw.split_inclusive(|byte| *byte == b'\n') {
            if line.starts_with(b"--- ") {
                return raw[offset..].to_vec();
            }
            offset += line.len();
        }
        Vec::new()
    }

    /// `--- `/`+++ ` 行的**原始**字节（含行尾补的 TAB，不含换行）。
    fn filepair_line(body: &[u8], index: usize) -> &[u8] {
        let line = body
            .split_inclusive(|byte| *byte == b'\n')
            .nth(index)
            .expect("filepair line");
        line.strip_suffix(b"\n").unwrap_or(line)
    }

    /// 从真实 git 的输出里取回它自己写出的两个 label（去掉行尾补的 TAB）。
    /// 测试不许硬编码期望字节，label 也必须是 git 的产物。
    fn git_labels(raw: &[u8]) -> (String, String) {
        let body = from_filepair(raw);
        let strip = |line: &[u8]| {
            let text = line.strip_suffix(b"\t").unwrap_or(line);
            assert!(text.starts_with(b"--- ") || text.starts_with(b"+++ "));
            String::from_utf8(text[4..].to_vec()).expect("label is ASCII once C-quoted")
        };
        (strip(filepair_line(&body, 0)), strip(filepair_line(&body, 1)))
    }

    /// 把 `dir` 下的相对路径（原始字节）拼成 `PathBuf`。
    fn path_in(dir: &Path, bytes: &[u8]) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            dir.join(OsStr::from_bytes(bytes))
        }
        #[cfg(not(unix))]
        {
            dir.join(String::from_utf8_lossy(bytes).into_owned())
        }
    }

    fn ours(old: &[u8], new: &[u8], context: usize) -> Vec<u8> {
        unified_bytes(old, new, "a/x", "b/y", context).expect("unified_bytes failed")
    }

    /// 差分断言：逐字节比，失败时打印双方原文（含转义），方便复核。
    fn assert_matches_git(old: &[u8], new: &[u8], context: usize) {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("x"), old).unwrap();
        fs::write(dir.path().join("y"), new).unwrap();
        let want = git_body(dir.path(), context, "x", "y");
        let got = ours(old, new, context);
        assert!(
            got == want,
            "diff mismatch (context={context})\nold = {old:?}\nnew = {new:?}\n--- git ---\n{}\n--- mg ---\n{}",
            String::from_utf8_lossy(&want),
            String::from_utf8_lossy(&got)
        );
    }

    fn lines(items: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for item in items {
            out.extend_from_slice(item.as_bytes());
            out.push(b'\n');
        }
        out
    }

    #[test]
    fn identical_files_produce_no_output() {
        let text = lines(&["a", "b", "c"]);
        assert!(ours(&text, &text, 3).is_empty());
        assert!(ours(b"", b"", 3).is_empty());
        assert_eq!(
            hunks(&myers::myers(b"x\n", b"x\n").unwrap(), b"x\n", b"x\n", 3).unwrap(),
            Vec::new()
        );
    }

    #[test]
    fn empty_versus_non_empty() {
        for context in [0usize, 1, 3] {
            assert_matches_git(b"", &lines(&["a", "b", "c"]), context);
            assert_matches_git(&lines(&["a", "b", "c"]), b"", context);
        }
    }

    #[test]
    fn pure_insert_and_delete() {
        let base = lines(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        for context in [0usize, 1, 3] {
            assert_matches_git(
                &base,
                &lines(&["a", "b", "c", "X", "d", "e", "f", "g", "h"]),
                context,
            );
            assert_matches_git(
                &lines(&["a", "b", "c", "X", "d", "e", "f", "g", "h"]),
                &base,
                context,
            );
            assert_matches_git(
                &base,
                &lines(&["X", "a", "b", "c", "d", "e", "f", "g", "h"]),
                context,
            );
            assert_matches_git(
                &base,
                &lines(&["a", "b", "c", "d", "e", "f", "g", "h", "X"]),
                context,
            );
        }
    }

    #[test]
    fn middle_modification() {
        let base = lines(&["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]);
        let changed = lines(&["1", "2", "3", "4", "5", "six", "7", "8", "9", "10"]);
        for context in [0usize, 1, 2, 3] {
            assert_matches_git(&base, &changed, context);
        }
    }

    #[test]
    fn multiple_hunks() {
        let base = lines(&[
            "l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9", "l10", "l11", "l12", "l13", "l14",
        ]);
        let changed = lines(&[
            "l1", "X2", "l3", "l4", "l5", "l6", "l7", "l8", "l9", "l10", "l11", "X12", "l13", "l14",
        ]);
        for context in [0usize, 1, 2, 3] {
            assert_matches_git(&base, &changed, context);
        }
    }

    #[test]
    fn adjacent_hunks_merge_at_2x_context() {
        let base = lines(&["l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8"]);
        // 两个变更块在旧文件里相隔 3 行：ctx<=1 时分开，ctx>=2 时合并。
        let changed = lines(&["l1", "X2", "l3", "l4", "l5", "X6", "l7", "l8"]);
        for context in [0usize, 1, 2, 3] {
            assert_matches_git(&base, &changed, context);
        }
        // 相隔 1 行：ctx=1 时 2*ctx=2 >= 1，必须合并。
        let changed2 = lines(&["l1", "X2", "l3", "X4", "l5", "l6", "l7", "l8"]);
        assert_matches_git(&base, &changed2, 1);
        // 长变更块紧邻下一个变更块（distance 的计算用的是「块起点」而不是「块终点」）。
        let changed3 = lines(&["l1", "X2", "X3", "l4", "X5", "l6", "l7", "l8"]);
        for context in [0usize, 1, 2] {
            assert_matches_git(&base, &changed3, context);
        }
    }

    #[test]
    fn missing_final_newline() {
        for context in [0usize, 1, 3] {
            // 两侧都没有末尾换行。
            assert_matches_git(b"abc", b"abd", context);
            // 只有旧侧没有末尾换行。
            assert_matches_git(b"abc", b"abc\n", context);
            // 只有新侧没有末尾换行。
            assert_matches_git(b"abc\n", b"abc", context);
            // 无换行的行作为**后置上下文**出现在 hunk 里。
            assert_matches_git(b"a\nX\nb", b"a\nY\nb", context);
            // 无换行的行作为前置上下文。
            assert_matches_git(b"a\nb", b"a\nb\nc\n", context);
        }
    }

    #[test]
    fn hunk_headers_where_git_prints_the_function_suffix() {
        // hunk 不在文件第 1 行时，git 会在 `@@ .. @@` 之后补一个「函数行」后缀
        // （`def_ff`：最近的一行以 [A-Za-z_$] 开头者）。
        let base = lines(&["l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9", "l10"]);
        let changed = lines(&[
            "l1", "l2", "l3", "l4", "l5", "l6", "CHANGED", "l8", "l9", "l10",
        ]);
        for context in [0usize, 1, 2, 3] {
            assert_matches_git(&base, &changed, context);
        }
        // 往上找时跳过空行与缩进行。
        let base2 = lines(&["    indented", "", "b", "c", "d"]);
        let changed2 = lines(&["    indented", "", "b", "C", "d"]);
        assert_matches_git(&base2, &changed2, 0);
        // 多 hunk 时每个 hunk 各自算后缀。
        let base3 = lines(&["f1", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]);
        let changed3 = lines(&["f1", "a", "B", "c", "d", "e", "f", "g", "H", "i", "j"]);
        assert_matches_git(&base3, &changed3, 0);
    }

    #[test]
    fn cjk_and_carriage_returns() {
        for context in [0usize, 1, 3] {
            assert_matches_git(
                "你好\n世界\n".as_bytes(),
                "你好\n地球\n".as_bytes(),
                context,
            );
            assert_matches_git(b"a\r\nb\r\nc\r\n", b"a\r\nB\r\nc\r\n", context);
            assert_matches_git(b"a\rb\nc\n", b"a\rb\nC\n", context);
            assert_matches_git(b"\xe4\xbd\xa0\n", b"\xe4\xbd\xa0\xe5\xa5\xbd\n", context);
        }
    }

    #[test]
    fn duplicate_lines_slide_like_git() {
        // 相邻行完全相同的时候，git 会把变更块「滑动」到最合适的位置
        // （`xdl_change_compact` + 缩进启发式）。这些是最容易与教科书 Myers 分叉的输入。
        assert_matches_git(
            &lines(&["a", "", "b", "c", "d"]),
            &lines(&["a", "b", "c", "d", ""]),
            0,
        );
        assert_matches_git(&lines(&["       }"]), &lines(&["   ", "}", "x", "y"]), 0);
        assert_matches_git(&lines(&["a", "a", "a", "b"]), &lines(&["a", "b"]), 0);
        assert_matches_git(&lines(&["a", "b"]), &lines(&["a", "a", "a", "b"]), 0);
        // 触发缩进启发式（git 2.14+ 默认开启）的那类输入。
        assert_matches_git(
            b"\nc\n  x\n  x\n\ty\n\ty\n  x\nb\n",
            b"   }\na\n  x\nb\n  x\nb\n  x\nb\n\ty\nb\n",
            0,
        );
        assert_matches_git(
            b" 1\n 2\n 3\n 4\n 5\n 6\n 7\n 8\n    }\n  };\n  return;\n}\n",
            b" 1\n 2\n 3\n 4\n 5\n 6\n  }\n  ;\n  return;\n}\n",
            3,
        );
    }

    #[test]
    fn random_small_inputs_match_real_git() {
        // 确定性 LCG（不引入新依赖），覆盖「小字母表 + 空行 + 缩进行」这类
        // 容易踩到剪枝/压缩/缩进启发式的输入。
        const ALPHABET: [&str; 8] = ["a", "b", "", "  x", "\ty", "c", "   }", "d"];
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |bound: usize| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as usize) % bound
        };
        for case in 0..250usize {
            let context = case % 4;
            let n1 = next(12);
            let n2 = next(12);
            let mut old = Vec::new();
            for _ in 0..n1 {
                old.extend_from_slice(ALPHABET[next(ALPHABET.len())].as_bytes());
                old.push(b'\n');
            }
            let mut new = Vec::new();
            for _ in 0..n2 {
                new.extend_from_slice(ALPHABET[next(ALPHABET.len())].as_bytes());
                new.push(b'\n');
            }
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("x"), &old).unwrap();
            fs::write(dir.path().join("y"), &new).unwrap();
            let want = git_body(dir.path(), context, "x", "y");
            let got = ours(&old, &new, context);
            assert!(
                got == want,
                "case {case} (context={context})\nold = {old:?}\nnew = {new:?}\n--- git ---\n{}\n--- mg ---\n{}",
                String::from_utf8_lossy(&want),
                String::from_utf8_lossy(&got)
            );
        }
    }

    #[test]
    fn hunks_api_reports_empty_ranges_at_the_insertion_point() {
        // 新文件（旧侧为空）：old_start 是插入位置对应的 1-based 行号，打印时减 1。
        let new = lines(&["a", "b"]);
        let edits = myers::myers(b"", &new).unwrap();
        let hunks = hunks(&edits, b"", &new, 3).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!((hunks[0].old_start, hunks[0].old_len), (1, 0));
        assert_eq!((hunks[0].new_start, hunks[0].new_len), (1, 2));
        assert_eq!(
            hunks[0].lines,
            vec![
                HunkLine::Added(b"a\n".to_vec()),
                HunkLine::Added(b"b\n".to_vec())
            ]
        );
        assert!(ours(b"", &new, 3).starts_with(b"--- a/x\n+++ b/y\n@@ -0,0 +1,2 @@\n+a\n+b\n"));
    }

    #[test]
    fn non_utf8_content_is_rejected_by_the_string_api_only() {
        let old = b"\xff\xfe\n";
        let new = b"\xff\xfd\n";
        assert!(unified(old, new, "a/x", "b/y", 3).is_err());
        assert!(!ours(old, new, 3).is_empty());
    }

    #[test]
    fn controller_owned_diff_texts_matches_real_git() {
        // 验证者会直接调 `minigit::diff::diff_texts`（mod.rs 里的转发），
        // 所以这条入口也要与真实 git 逐字节一致。
        let cases: [(&[u8], &[u8], usize); 6] = [
            (b"", &lines(&["a", "b", "c"]), 3),
            (&lines(&["a", "b", "c"]), b"", 0),
            (
                &lines(&["l1", "l2", "l3", "l4", "l5"]),
                &lines(&["l1", "X", "l3", "l4", "l5"]),
                1,
            ),
            (
                &lines(&["f1", "a", "b", "c", "d", "e"]),
                &lines(&["f1", "a", "B", "c", "d", "e"]),
                0,
            ),
            (b"abc", b"abd", 3),
            (
                &lines(&["a", "b", "c", "d"]),
                &lines(&["a", "X", "c", "Y"]),
                2,
            ),
        ];
        for (old, new, context) in cases {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("x"), old).unwrap();
            fs::write(dir.path().join("y"), new).unwrap();
            let want = String::from_utf8(git_body(dir.path(), context, "x", "y")).unwrap();
            let got = crate::diff::diff_texts(old, new, "a/x", "b/y", context).unwrap();
            assert_eq!(got, want, "diff_texts mismatch (context={context})");
        }
    }

    // -----------------------------------------------------------------------
    // `--- ` / `+++ ` 行的 TAB 填充（git：`strchr(label, ' ') ? "\t" : ""`）
    // -----------------------------------------------------------------------

    /// 名字类别矩阵：每类给「旧侧名 / 新侧名」两个不同的名字（内容也不同，
    /// 否则 git 不会输出 filepair）。
    fn name_cases() -> Vec<(&'static str, Vec<u8>, Vec<u8>)> {
        let mut cases: Vec<(&'static str, Vec<u8>, Vec<u8>)> = vec![
            (
                "no space",
                b"plain_a.txt".to_vec(),
                b"plain_b.txt".to_vec(),
            ),
            ("space", b"sp ace_a.txt".to_vec(), b"sp ace_b.txt".to_vec()),
            (
                "space in a directory component",
                b"dir x/a.txt".to_vec(),
                b"dir x/b.txt".to_vec(),
            ),
            (
                "trailing space before the extension",
                b"trail _a.txt".to_vec(),
                b"trail _b.txt".to_vec(),
            ),
            (
                "name ends with a space",
                b"end_a ".to_vec(),
                b"end_b ".to_vec(),
            ),
            (
                "double quote",
                b"qu\"ote_a.txt".to_vec(),
                b"qu\"ote_b.txt".to_vec(),
            ),
            (
                "backslash",
                b"back\\slash_a.txt".to_vec(),
                b"back\\slash_b.txt".to_vec(),
            ),
            ("tab", b"ta\tb_a.txt".to_vec(), b"ta\tb_b.txt".to_vec()),
            (
                "CJK",
                "中文_a.txt".as_bytes().to_vec(),
                "中文_b.txt".as_bytes().to_vec(),
            ),
            (
                "quote and space",
                b"qu\"o te_a.txt".to_vec(),
                b"qu\"o te_b.txt".to_vec(),
            ),
        ];
        #[cfg(unix)]
        {
            // 非 UTF-8 的名字会被 C-quote 成纯 ASCII（`\377`），所以取回来的 label
            // 仍然可以当 `&str` 传进 `unified_bytes`。
            cases.push((
                "non-UTF-8 bytes",
                b"raw\xff_a.txt".to_vec(),
                b"raw\xff_b.txt".to_vec(),
            ));
            cases.push((
                "non-UTF-8 bytes and a space",
                b"raw\xff a.txt".to_vec(),
                b"raw\xff b.txt".to_vec(),
            ));
        }
        cases
    }

    /// 逐字节对拍：`--- `/`+++ ` 行**当且仅当该 label 含空格字节**时补 TAB。
    ///
    /// 每类名字都在真实 git 里跑 `diff --no-index -U0 / -U3`，label 也从 git 的
    /// 输出里取回来再喂给引擎 —— 期望字节全部来自运行时的 git 进程。
    #[test]
    fn filepair_lines_are_tab_padded_only_when_the_label_contains_a_space() {
        let old = b"line1\nline2\nline3\n";
        let new = b"line1\nCHANGED\nline3\n";
        for (class, name_a, name_b) in name_cases() {
            let dir = tempfile::tempdir().expect("tempdir");
            for (name, bytes) in [(&name_a, &old[..]), (&name_b, &new[..])] {
                let path = path_in(dir.path(), name);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).expect("create parent dir");
                }
                fs::write(&path, bytes).expect("write fixture");
            }

            for context in [0usize, 3] {
                let raw =
                    git_no_index(dir.path(), context, os_str(&name_a), os_str(&name_b));
                assert!(
                    raw.starts_with(b"diff --git "),
                    "{class}: expected a filepair header, got {:?}",
                    String::from_utf8_lossy(&raw)
                );
                // `diff --git` 行走的是另一条 emit 路径：从不补 TAB
                // （名字里的 TAB 会被 C-quote 转义成两个字节 `\` `t`）。
                let header = raw.split_inclusive(|b| *b == b'\n').next().unwrap();
                assert!(
                    !header.contains(&b'\t'),
                    "{class}: `diff --git` line must never be padded: {:?}",
                    String::from_utf8_lossy(header)
                );

                let (a_label, b_label) = git_labels(&raw);
                let want = from_filepair(&raw);
                let got = unified_bytes(old, new, &a_label, &b_label, context)
                    .expect("unified_bytes failed");

                // 规则本身：git 补 TAB 当且仅当该 label 含空格字节（逐 label 判定）。
                for (index, label) in [(0usize, &a_label), (1usize, &b_label)] {
                    let line = filepair_line(&want, index);
                    assert_eq!(
                        line.contains(&b'\t'),
                        label.contains(' '),
                        "{class} -U{context} filepair line {index}: label {label:?} -> {:?}",
                        String::from_utf8_lossy(line)
                    );
                }

                assert!(
                    got == want,
                    "{class} -U{context}\nlabels = {a_label:?} / {b_label:?}\n--- git ---\n{}\n--- mg ---\n{}",
                    String::from_utf8_lossy(&want),
                    String::from_utf8_lossy(&got)
                );
            }
        }
    }

    /// 新文件 / 删除文件：`/dev/null` 那一侧不含空格 → 不补；只有带名字的那一侧补。
    /// 真值仍然来自真实仓库里的 git。
    #[test]
    fn null_side_is_never_padded_in_a_real_repo() {
        let repo = tempfile::tempdir().expect("tempdir");
        let dir = repo.path();
        git_repo(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("seed.txt"), b"seed\n").unwrap();
        git_repo(dir, &["add", "seed.txt"]);
        git_repo(
            dir,
            &[
                "-c",
                "user.name=V",
                "-c",
                "user.email=v@e",
                "commit",
                "-qm",
                "base",
            ],
        );

        let content: &[u8] = b"line1\nline2\n";
        fs::write(dir.join("new file.txt"), content).unwrap();
        git_repo(dir, &["add", "new file.txt"]);
        let raw = git_repo(dir, &["diff", "--no-renames", "--staged", "--unified=3"]);
        let (a_label, b_label) = git_labels(&raw);
        assert_eq!(a_label, "/dev/null");
        assert_eq!(b_label, "b/new file.txt");
        let got = unified_bytes(b"", content, &a_label, &b_label, 3).expect("unified_bytes failed");
        assert_eq!(got, from_filepair(&raw));
        assert_eq!(filepair_line(&got, 0).to_vec(), b"--- /dev/null".to_vec());
        assert_eq!(
            filepair_line(&got, 1).to_vec(),
            format!("+++ {b_label}\t").into_bytes()
        );

        // 反向：删掉一个名字含空格的文件 → 只有 `---` 一侧补。
        git_repo(
            dir,
            &[
                "-c",
                "user.name=V",
                "-c",
                "user.email=v@e",
                "commit",
                "-qm",
                "add spaced file",
            ],
        );
        git_repo(dir, &["rm", "-q", "new file.txt"]);
        let raw = git_repo(dir, &["diff", "--no-renames", "--staged", "--unified=3"]);
        let (a_label, b_label) = git_labels(&raw);
        assert_eq!(a_label, "a/new file.txt");
        assert_eq!(b_label, "/dev/null");
        let got =
            unified_bytes(content, b"", &a_label, &b_label, 3).expect("unified_bytes failed");
        assert_eq!(got, from_filepair(&raw));
        assert_eq!(
            filepair_line(&got, 0).to_vec(),
            format!("--- {a_label}\t").into_bytes()
        );
        assert_eq!(filepair_line(&got, 1).to_vec(), b"+++ /dev/null".to_vec());
    }
}
