//! 三方合并（文件级）。**T10（opencode）实现范围**。
//!
//! 规则：
//! * `base == ours` → 取 theirs；`base == theirs` → 取 ours；`ours == theirs` → 取任一。
//! * base 为 `None`（两个新增文件）时，内容相同则干净，否则冲突。
//! * 冲突输出必须与 git 一致（diff3 风格：含 `||||||| base` 段）。
//! * 冲突输出**必须带末尾换行**，且 marker 长度固定为 7 个字符 + 标签。
//! * 二进制内容直接判冲突，不插 marker。
//!
//! 验收：与 `git merge` 的冲突文件内容逐字节一致；无冲突时 tree 相同。
//!
//! # 实现说明
//!
//! 本文件是 git 的 `xdiff/xmerge.c` + `xdiff/xdiffi.c` 里
//! `xdl_merge` / `xdl_do_merge` / `xdl_fill_merge_buffer` / `xdl_refine_conflicts`
//! / `xdl_simplify_non_conflicts` 的逐函数移植。行级 diff **不重新实现 Myers**：
//! 直接调用 T6 的 [`crate::diff::myers::myers`]（它就是 `xdl_do_diff` +
//! `xdl_change_compact` + `xdl_build_script` 的移植），再把「编辑脚本」转成
//! xmerge 需要的 `(i1, chg1, i2, chg2)` 变更记录。
//!
//! 由此得到的对齐、冲突判定、marker 字节、`\ No newline` / CRLF 处理都与真实 git
//! 同源。`merge_blobs` 使用与 `git merge-file` 相同的 `level = XDL_MERGE_ZEALOUS_ALNUM`；
//! CLI 侧真正合并时用 `git merge`（`ll-merge.c`）的 `level = XDL_MERGE_ZEALOUS`，
//! 见 [`merge_blobs_with_level`]。
//!
//! 已知差异（都只在极端输入下才可能出现，验收场景不覆盖）：
//! * `git merge-file` 传给 xdiff 的 `xpp.flags = 0`（不带 `XDF_INDENT_HEURISTIC`），
//!   而 T6 的 `myers` 统一启用缩进启发式（`git diff` 的默认）。只有在**缩进启发式
//!   真的改变了变更块位置的输入**上才可能分叉；本文件的差分测试在随机语料上对拍。
//! * 不做 rename 检测、不做 `-X ours/theirs`、不做 octopus。

use crate::diff::myers::{self, Op};
use crate::diff::split_lines;
use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// 能自动合并的结果内容。
    Clean(Vec<u8>),
    /// 带冲突标记的合并结果（仍需写入工作区，并写 index stage 1/2/3）。
    Conflict(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct MergeLabels {
    pub ours: String,
    pub base: String,
    pub theirs: String,
    /// true = 输出 diff3 风格的 `||||||| base` 段。
    pub diff3_style: bool,
}

impl Default for MergeLabels {
    fn default() -> Self {
        MergeLabels {
            ours: "ours".to_string(),
            base: "base".to_string(),
            theirs: "theirs".to_string(),
            diff3_style: false,
        }
    }
}

// xdiff 的合并简化级别（xdiff/xdiff.h）。
const LEVEL_MINIMAL: i32 = 0;
const LEVEL_EAGER: i32 = 1;
const LEVEL_ZEALOUS: i32 = 2;
const LEVEL_ZEALOUS_ALNUM: i32 = 3;

/// `DEFAULT_CONFLICT_MARKER_SIZE`。
const MARKER_SIZE: usize = 7;

/// 二进制探测窗口（`FIRST_FEW_BYTES`）。
const BINARY_WINDOW: usize = 8000;

/// 一个变更记录，对应 xdiff 的 `xdchange_t` / `xdmerge_t` 的坐标：
/// `i1/chg1` 是「基准文件（preimage）」里的区间，`i2/chg2` 是某一侧 postimage 里的区间。
#[derive(Debug, Clone, Copy)]
struct Change {
    i1: i64,
    chg1: i64,
    i2: i64,
    chg2: i64,
}

/// `xdmerge_t`：一段待输出/待判定的合并区域。
#[derive(Debug, Clone, Copy)]
struct Block {
    /// 0 = 冲突；1 = 取 side1；2 = 取 side2；4 = 两侧改动完全相同（不输出、也不计冲突）。
    mode: i32,
    i0: i64,
    chg0: i64,
    i1: i64,
    chg1: i64,
    i2: i64,
    chg2: i64,
}

/// 文件级三方合并（`git merge-file` 默认：`level = XDL_MERGE_ZEALOUS_ALNUM`）。
pub fn merge_blobs(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
    labels: &MergeLabels,
) -> Result<Merged> {
    let level = if labels.diff3_style {
        LEVEL_EAGER
    } else {
        LEVEL_ZEALOUS_ALNUM
    };
    merge_with_level(base, ours, theirs, labels, level)
}

/// 与 [`merge_blobs`] 相同，但显式指定简化级别。CLI 的正常合并用
/// `LEVEL_ZEALOUS`（对应 `git merge` 走的 `ll-merge.c` → `xdl_merge`）。
pub(crate) fn merge_blobs_with_level(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
    labels: &MergeLabels,
    level: i32,
) -> Result<Merged> {
    merge_with_level(base, ours, theirs, labels, level)
}

fn merge_with_level(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
    labels: &MergeLabels,
    level: i32,
) -> Result<Merged> {
    // 1) 同一内容 / 只有一侧改动：直接取另一边（git 的 `!xscr` 短路）。
    if ours == theirs {
        return Ok(Merged::Clean(ours.to_vec()));
    }
    if let Some(base) = base {
        if base == ours {
            return Ok(Merged::Clean(theirs.to_vec()));
        }
        if base == theirs {
            return Ok(Merged::Clean(ours.to_vec()));
        }
    }

    // 2) 任一侧二进制 → 保留 ours、判冲突（ll-merge 的 `ll_binary_merge`）。
    if base.is_some_and(is_binary) || is_binary(ours) || is_binary(theirs) {
        return Ok(Merged::Conflict(ours.to_vec()));
    }

    let base = base.unwrap_or(b"");

    // 3) 两侧相对 base 的编辑脚本（xdl_do_diff + xdl_change_compact）。
    let script_ours = script(base, ours)?;
    let script_theirs = script(base, theirs)?;
    if script_ours.is_empty() {
        return Ok(Merged::Clean(theirs.to_vec()));
    }
    if script_theirs.is_empty() {
        return Ok(Merged::Clean(ours.to_vec()));
    }

    let base_lines = split_lines(base);
    let ours_lines = split_lines(ours);
    let theirs_lines = split_lines(theirs);

    // 4) 对齐两个脚本（xdl_do_merge 的主循环 + 收尾）。
    let mut blocks = combine(
        &script_ours,
        &script_theirs,
        &base_lines,
        &ours_lines,
        &theirs_lines,
        level,
    );

    // diff3 风格时 level 已被钳到 EAGER（见 xdl_do_merge 开头），不会走简化。
    if level >= LEVEL_ZEALOUS {
        refine_conflicts(&mut blocks, &ours_lines, &theirs_lines)?;
        simplify_non_conflicts(&mut blocks, &ours_lines, level > LEVEL_ZEALOUS);
    }

    let conflicts = blocks.iter().filter(|block| block.mode == 0).count();
    let out = fill_merge_buffer(&blocks, &base_lines, &ours_lines, &theirs_lines, labels);

    if conflicts == 0 {
        Ok(Merged::Clean(out))
    } else {
        Ok(Merged::Conflict(out))
    }
}

/// `buffer_is_binary`：前 8000 字节含 NUL。
fn is_binary(data: &[u8]) -> bool {
    data.iter().take(BINARY_WINDOW).any(|byte| *byte == 0)
}

/// 把 T6 的编辑脚本转成 xmerge 的变更记录：每个「变更块」=
/// `(base_start, base_removed, side_start, side_added)`。
fn script(base: &[u8], side: &[u8]) -> Result<Vec<Change>> {
    let edits = myers::myers(base, side)?;
    let mut out: Vec<Change> = Vec::new();
    let mut current: Option<Change> = None;
    for edit in &edits {
        if edit.op == Op::Equal {
            if let Some(done) = current.take() {
                out.push(done);
            }
            continue;
        }
        let old_end = edit.old.end as i64;
        let new_end = edit.new.end as i64;
        match current.as_mut() {
            None => {
                current = Some(Change {
                    i1: edit.old.start as i64,
                    chg1: old_end - edit.old.start as i64,
                    i2: edit.new.start as i64,
                    chg2: new_end - edit.new.start as i64,
                })
            }
            Some(change) => {
                // 变更块里 Delete 在 Insert 之前：分别取两侧区间的最大长度。
                change.chg1 = change.chg1.max(old_end - change.i1);
                change.chg2 = change.chg2.max(new_end - change.i2);
            }
        }
    }
    if let Some(done) = current {
        out.push(done);
    }
    Ok(out)
}

/// `xdl_append_merge`：把新块并入上一个块（重叠/相邻时）或追加。
#[allow(clippy::too_many_arguments)]
fn append(
    blocks: &mut Vec<Block>,
    mode: i32,
    i0: i64,
    chg0: i64,
    i1: i64,
    chg1: i64,
    i2: i64,
    chg2: i64,
) {
    if let Some(last) = blocks.last_mut() {
        if i1 <= last.i1 + last.chg1 || i2 <= last.i2 + last.chg2 {
            if mode != last.mode {
                last.mode = 0;
            }
            last.chg0 = i0 + chg0 - last.i0;
            last.chg1 = i1 + chg1 - last.i1;
            last.chg2 = i2 + chg2 - last.i2;
            return;
        }
    }
    blocks.push(Block {
        mode,
        i0,
        chg0,
        i1,
        chg1,
        i2,
        chg2,
    });
}

/// `xdl_do_merge` 的主循环与两段收尾。
fn combine(
    ours_script: &[Change],
    theirs_script: &[Change],
    base_lines: &[&[u8]],
    ours_lines: &[&[u8]],
    theirs_lines: &[&[u8]],
    level: i32,
) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let (mut p, mut q) = (0usize, 0usize);

    while p < ours_script.len() && q < theirs_script.len() {
        let x1 = ours_script[p];
        let x2 = theirs_script[q];

        if x1.i1 + x1.chg1 < x2.i1 {
            append(
                &mut blocks,
                1,
                x1.i1,
                x1.chg1,
                x1.i2,
                x1.chg2,
                x2.i2 - x2.i1 + x1.i1,
                x1.chg1,
            );
            p += 1;
            continue;
        }
        if x2.i1 + x2.chg1 < x1.i1 {
            append(
                &mut blocks,
                2,
                x2.i1,
                x2.chg1,
                x1.i2 - x1.i1 + x2.i1,
                x2.chg1,
                x2.i2,
                x2.chg2,
            );
            q += 1;
            continue;
        }

        let identical = x1.i1 == x2.i1
            && x1.chg1 == x2.chg1
            && x1.chg2 == x2.chg2
            && lines_equal(ours_lines, theirs_lines, x1.i2, x2.i2, x1.chg2);

        if level == LEVEL_MINIMAL || !identical {
            let off = x1.i1 - x2.i1;
            let ffo = off + x1.chg1 - x2.chg1;

            let mut i0 = x1.i1;
            let mut i1 = x1.i2;
            let mut i2 = x2.i2;
            if off > 0 {
                i0 -= off;
                i1 -= off;
            } else {
                i2 += off;
            }
            let mut chg0 = x1.i1 + x1.chg1 - i0;
            let mut chg1 = x1.i2 + x1.chg2 - i1;
            let mut chg2 = x2.i2 + x2.chg2 - i2;
            if ffo < 0 {
                chg0 -= ffo;
                chg1 -= ffo;
            } else {
                chg2 += ffo;
            }
            append(&mut blocks, 0, i0, chg0, i1, chg1, i2, chg2);
        }

        let end1 = x1.i1 + x1.chg1;
        let end2 = x2.i1 + x2.chg1;
        if end1 >= end2 {
            q += 1;
        }
        if end2 >= end1 {
            p += 1;
        }
    }

    let base_nrec = base_lines.len() as i64;
    while p < ours_script.len() {
        let x1 = ours_script[p];
        let i2 = x1.i1 + theirs_lines.len() as i64 - base_nrec;
        append(&mut blocks, 1, x1.i1, x1.chg1, x1.i2, x1.chg2, i2, x1.chg1);
        p += 1;
    }
    while q < theirs_script.len() {
        let x2 = theirs_script[q];
        let i1 = x2.i1 + ours_lines.len() as i64 - base_nrec;
        append(&mut blocks, 2, x2.i1, x2.chg1, i1, x2.chg1, x2.i2, x2.chg2);
        q += 1;
    }
    blocks
}

/// `xdl_refine_conflicts`：对每个冲突块再跑一次 ours-vs-theirs 的 diff，
/// 只把真正不同的片段留成冲突（默认 merge 风格会走这里）。
fn refine_conflicts(
    blocks: &mut Vec<Block>,
    ours_lines: &[&[u8]],
    theirs_lines: &[&[u8]],
) -> Result<()> {
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    for block in blocks.iter().copied() {
        if block.mode != 0 || block.chg1 == 0 || block.chg2 == 0 {
            out.push(block);
            continue;
        }
        let ours_side = join_lines(ours_lines, block.i1, block.chg1);
        let theirs_side = join_lines(theirs_lines, block.i2, block.chg2);
        let refined = script(&ours_side, &theirs_side)?;
        if refined.is_empty() {
            // 两侧改动完全相同 → 既不是冲突，也不需要输出（随后从 ours 取上下文）。
            out.push(Block { mode: 4, ..block });
            continue;
        }
        for (index, fragment) in refined.iter().enumerate() {
            out.push(Block {
                mode: 0,
                i0: if index == 0 { block.i0 } else { 0 },
                chg0: if index == 0 { block.chg0 } else { 0 },
                i1: fragment.i1 + block.i1,
                chg1: fragment.chg1,
                i2: fragment.i2 + block.i2,
                chg2: fragment.chg2,
            });
        }
    }
    *blocks = out;
    Ok(())
}

/// `xdl_simplify_non_conflicts`：两个冲突之间隔得太近（≤3 行，或在不含字母数字时
/// 任意间隔——取决于 level）就并成一个冲突。
fn simplify_non_conflicts(
    blocks: &mut Vec<Block>,
    ours_lines: &[&[u8]],
    simplify_if_no_alnum: bool,
) {
    let mut index = 0usize;
    while index + 1 < blocks.len() {
        let current = blocks[index];
        let next = blocks[index + 1];
        let begin = current.i1 + current.chg1;
        let end = next.i1;
        let gap = end - begin;
        let should_merge = current.mode == 0
            && next.mode == 0
            && (gap <= 3 || (simplify_if_no_alnum && !lines_contain_alnum(ours_lines, begin, gap)));
        if should_merge {
            blocks[index].chg1 = next.i1 + next.chg1 - current.i1;
            blocks[index].chg2 = next.i2 + next.chg2 - current.i2;
            blocks.remove(index + 1);
        } else {
            index += 1;
        }
    }
}

/// `xdl_fill_merge_buffer`：把 block 列表渲染成输出字节。
fn fill_merge_buffer(
    blocks: &[Block],
    base_lines: &[&[u8]],
    ours_lines: &[&[u8]],
    theirs_lines: &[&[u8]],
    labels: &MergeLabels,
) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut i: i64 = 0;
    for block in blocks {
        if block.mode == 0 {
            fill_conflict(
                &mut out,
                block,
                i,
                base_lines,
                ours_lines,
                theirs_lines,
                labels,
            );
        } else if block.mode & 3 != 0 {
            copy_lines(&mut out, ours_lines, i, block.i1 - i, false, false);
            if block.mode & 1 != 0 {
                let needs_cr = is_cr_needed(ours_lines, theirs_lines, base_lines, block);
                copy_lines(
                    &mut out,
                    ours_lines,
                    block.i1,
                    block.chg1,
                    needs_cr,
                    block.mode & 2 != 0,
                );
            }
            if block.mode & 2 != 0 {
                copy_lines(&mut out, theirs_lines, block.i2, block.chg2, false, false);
            }
        } else {
            // mode == 4：两侧改动相同，这段内容稍后作为上下文从 ours 输出。
            continue;
        }
        i = block.i1 + block.chg1;
    }
    copy_lines(
        &mut out,
        ours_lines,
        i,
        ours_lines.len() as i64 - i,
        false,
        false,
    );
    out
}

/// `fill_conflict_hunk`。
#[allow(clippy::too_many_arguments)]
fn fill_conflict(
    out: &mut Vec<u8>,
    block: &Block,
    cursor: i64,
    base_lines: &[&[u8]],
    ours_lines: &[&[u8]],
    theirs_lines: &[&[u8]],
    labels: &MergeLabels,
) {
    copy_lines(out, ours_lines, cursor, block.i1 - cursor, false, false);
    let needs_cr = is_cr_needed(ours_lines, theirs_lines, base_lines, block);

    push_marker(out, b'<', Some(&labels.ours), needs_cr);
    copy_lines(out, ours_lines, block.i1, block.chg1, needs_cr, true);

    if labels.diff3_style {
        push_marker(out, b'|', Some(&labels.base), needs_cr);
        copy_lines(out, base_lines, block.i0, block.chg0, needs_cr, true);
    }

    push_marker(out, b'=', None, needs_cr);
    copy_lines(out, theirs_lines, block.i2, block.chg2, needs_cr, true);
    push_marker(out, b'>', Some(&labels.theirs), needs_cr);
}

/// marker 行：`<<<<<<< name\n`（`name` 为空也会有一个空格，与 git 一致）。
fn push_marker(out: &mut Vec<u8>, marker: u8, name: Option<&str>, needs_cr: bool) {
    out.extend(std::iter::repeat(marker).take(MARKER_SIZE));
    if let Some(name) = name {
        out.push(b' ');
        out.extend_from_slice(name.as_bytes());
    }
    if needs_cr {
        out.push(b'\r');
    }
    out.push(b'\n');
}

/// `xdl_recs_copy`：复制 `lines[start..start+count]`；`add_nl` 时若最后一行没有
/// 换行就补一个（CRLF 场景补 `\r\n`）。
fn copy_lines(
    out: &mut Vec<u8>,
    lines: &[&[u8]],
    start: i64,
    count: i64,
    needs_cr: bool,
    add_nl: bool,
) {
    if count < 1 {
        return;
    }
    let start = start as usize;
    let count = count as usize;
    for line in &lines[start..start + count] {
        out.extend_from_slice(line);
    }
    if add_nl {
        let last = lines[start + count - 1];
        if last.is_empty() || last[last.len() - 1] != b'\n' {
            if needs_cr {
                out.push(b'\r');
            }
            out.push(b'\n');
        }
    }
}

/// `is_eol_crlf`：判断第 `i` 行结尾是否为 CRLF（返回 `-1` 表示无法判断）。
fn is_eol_crlf(lines: &[&[u8]], i: i64) -> i32 {
    let n = lines.len() as i64;
    if i < n - 1 {
        let line = lines[i as usize];
        return (line.len() > 1 && line[line.len() - 2] == b'\r') as i32;
    }
    if n == 0 {
        return -1;
    }
    let line = lines[i as usize];
    if !line.is_empty() && line[line.len() - 1] == b'\n' {
        return (line.len() > 1 && line[line.len() - 2] == b'\r') as i32;
    }
    if i == 0 {
        return -1;
    }
    let previous = lines[(i - 1) as usize];
    (previous.len() > 1 && previous[previous.len() - 2] == b'\r') as i32
}

/// `is_cr_needed`：链路是「ours 前一行 → theirs 前一行 → base 第一行」，
/// 任一环节返回 0 就停（`-1` 在 C 里是 truthy，会继续往下走）。
fn is_cr_needed(
    ours_lines: &[&[u8]],
    theirs_lines: &[&[u8]],
    base_lines: &[&[u8]],
    block: &Block,
) -> bool {
    let mut needs_cr = is_eol_crlf(ours_lines, if block.i1 > 0 { block.i1 - 1 } else { 0 });
    if needs_cr != 0 {
        needs_cr = is_eol_crlf(theirs_lines, if block.i2 > 0 { block.i2 - 1 } else { 0 });
    }
    if needs_cr != 0 {
        needs_cr = is_eol_crlf(base_lines, 0);
    }
    needs_cr > 0
}

/// `xdl_merge_cmp_lines`：比较两侧 postimage 的 `count` 行是否逐字节相同。
fn lines_equal(
    ours_lines: &[&[u8]],
    theirs_lines: &[&[u8]],
    ours_start: i64,
    theirs_start: i64,
    count: i64,
) -> bool {
    if count <= 0 {
        return true;
    }
    let ours_start = ours_start as usize;
    let theirs_start = theirs_start as usize;
    let count = count as usize;
    (0..count).all(|offset| ours_lines[ours_start + offset] == theirs_lines[theirs_start + offset])
}

/// `lines_contain_alnum`：区间内是否有任一 `isalnum` 字节（C locale）。
fn lines_contain_alnum(lines: &[&[u8]], start: i64, count: i64) -> bool {
    if count <= 0 {
        return false;
    }
    let start = start as usize;
    let count = count as usize;
    lines[start..start + count]
        .iter()
        .any(|line| line.iter().any(u8::is_ascii_alphanumeric))
}

/// 把若干行拼成连续字节（refine 时的 `t1`/`t2`）。
fn join_lines(lines: &[&[u8]], start: i64, count: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for line in &lines[start as usize..(start + count) as usize] {
        out.extend_from_slice(line);
    }
    out
}

#[cfg(test)]
mod tests {
    //! 真值只来自运行时的 `git merge-file`（两种风格），不硬编码期望字节。

    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn labels(diff3_style: bool) -> MergeLabels {
        MergeLabels {
            ours: "ours".to_string(),
            base: "base".to_string(),
            theirs: "theirs".to_string(),
            diff3_style,
        }
    }

    /// 跑 `git merge-file -p -L ours -L base -L theirs [--diff3] ours base theirs`。
    /// 返回 `(exit_code, stdout)`；exit code > 0 表示有冲突。
    fn git_merge_file(
        dir: &Path,
        base: &[u8],
        ours: &[u8],
        theirs: &[u8],
        diff3_style: bool,
    ) -> (i32, Vec<u8>) {
        std::fs::write(dir.join("base"), base).unwrap();
        std::fs::write(dir.join("ours"), ours).unwrap();
        std::fs::write(dir.join("theirs"), theirs).unwrap();
        let mut command = Command::new("git");
        command
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .args([
                "merge-file",
                "-p",
                "-L",
                "ours",
                "-L",
                "base",
                "-L",
                "theirs",
            ]);
        if diff3_style {
            command.arg("--diff3");
        }
        let output = command
            .args(["ours", "base", "theirs"])
            .output()
            .expect("spawn git merge-file");
        let code = output.status.code().unwrap_or(-1);
        (code, output.stdout)
    }

    fn bytes(merged: &Merged) -> &[u8] {
        match merged {
            Merged::Clean(bytes) | Merged::Conflict(bytes) => bytes,
        }
    }

    /// 逐字节对拍：内容与「是否冲突」都必须与真实 git 一致。
    fn assert_matches_git(base: &[u8], ours: &[u8], theirs: &[u8]) {
        for diff3_style in [false, true] {
            let dir = tempfile::tempdir().expect("tempdir");
            let labels = labels(diff3_style);
            let (code, want) = git_merge_file(dir.path(), base, ours, theirs, diff3_style);
            let merged = merge_blobs(Some(base), ours, theirs, &labels).expect("merge_blobs");
            let got = bytes(&merged);
            assert_eq!(
                got,
                want.as_slice(),
                "merge mismatch (diff3={diff3_style})\nbase   = {base:?}\nours   = {ours:?}\ntheirs = {theirs:?}\n--- git ---\n{}\n--- mg ---\n{}",
                String::from_utf8_lossy(&want),
                String::from_utf8_lossy(got),
            );
            assert_eq!(
                code > 0,
                matches!(merged, Merged::Conflict(_)),
                "conflict status mismatch (diff3={diff3_style}) base={base:?} ours={ours:?} theirs={theirs:?} git_exit={code}"
            );
        }
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
    fn identical_and_ancestor_shortcuts() {
        let base = lines(&["a", "b", "c"]);
        let other = lines(&["a", "B", "c"]);
        assert_eq!(
            merge_blobs(Some(&base), &other, &other, &labels(false)).unwrap(),
            Merged::Clean(other.clone())
        );
        assert_eq!(
            merge_blobs(Some(&base), &base, &other, &labels(false)).unwrap(),
            Merged::Clean(other.clone())
        );
        assert_eq!(
            merge_blobs(Some(&base), &other, &base, &labels(false)).unwrap(),
            Merged::Clean(other.clone())
        );
        assert_eq!(
            merge_blobs(Some(&base), &base, &base, &labels(false)).unwrap(),
            Merged::Clean(base.clone())
        );
        assert_matches_git(&base, &other, &other);
        assert_matches_git(&base, &base, &other);
        assert_matches_git(&base, &other, &base);
        assert_matches_git(&base, &base, &base);
    }

    #[test]
    fn both_added_uses_none_base() {
        let ours = lines(&["one"]);
        let theirs = lines(&["two"]);
        let dir = tempfile::tempdir().unwrap();
        let (code, want) = git_merge_file(dir.path(), b"", &ours, &theirs, false);
        let merged = merge_blobs(None, &ours, &theirs, &labels(false)).unwrap();
        assert_eq!(bytes(&merged), want.as_slice());
        assert_eq!(code > 0, matches!(merged, Merged::Conflict(_)));
        let dir = tempfile::tempdir().unwrap();
        let (code, want) = git_merge_file(dir.path(), b"", &ours, &theirs, true);
        let merged = merge_blobs(None, &ours, &theirs, &labels(true)).unwrap();
        assert_eq!(bytes(&merged), want.as_slice());
        assert_eq!(code > 0, matches!(merged, Merged::Conflict(_)));
        assert_eq!(
            merge_blobs(None, &ours, &ours, &labels(false)).unwrap(),
            Merged::Clean(ours.clone())
        );
    }

    #[test]
    fn scenario_matrix_matches_git() {
        let base = lines(&["1", "2", "3", "4", "5", "6", "7", "8"]);
        // 两侧不同位置改动（干净）
        assert_matches_git(
            &base,
            &lines(&["1", "X", "3", "4", "5", "6", "7", "8"]),
            &lines(&["1", "2", "3", "4", "5", "6", "7", "Y"]),
        );
        // 同一行两侧都改（冲突）
        assert_matches_git(
            &base,
            &lines(&["1", "2", "3", "OURS", "5", "6", "7", "8"]),
            &lines(&["1", "2", "3", "THEIRS", "5", "6", "7", "8"]),
        );
        // 一侧删、一侧改
        assert_matches_git(
            &base,
            &lines(&["1", "2", "4", "5", "6", "7", "8"]),
            &lines(&["1", "2", "3", "CHANGED", "5", "6", "7", "8"]),
        );
        // 相邻改动（间隔小，触发 refine/simplify）
        assert_matches_git(
            &base,
            &lines(&["1", "2", "X", "4", "5", "6", "7", "8"]),
            &lines(&["1", "2", "3", "Y", "5", "6", "7", "8"]),
        );
        // 两侧都新增同一文件（内容不同）→ 见 both_added；这里再对拍一个空 base 变体
        assert_matches_git(b"", &lines(&["a"]), &lines(&["b"]));
        assert_matches_git(b"", b"", b"");
        assert_matches_git(b"", &lines(&["a"]), b"");
        assert_matches_git(&lines(&["a", "b"]), b"", &lines(&["a", "c"]));
        assert_matches_git(&lines(&["a", "b"]), &lines(&["a"]), &lines(&["a", "B"]));
    }

    #[test]
    fn missing_final_newline_matches_git() {
        // 单侧/两侧都没有末尾换行
        assert_matches_git(b"a\nb", b"a\nB", b"a\nb");
        assert_matches_git(b"a\nb", b"a\nB\n", b"a\nb");
        assert_matches_git(b"a\nb\n", b"a\nB", b"a\nb\n");
        assert_matches_git(b"abc", b"abd", b"abX");
        // 冲突时一侧缺末尾换行
        assert_matches_git(b"1\n2\n3", b"1\nOURS\n3", b"1\nTHEIRS\n3");
        assert_matches_git(b"1\n2\n3\n", b"1\nOURS", b"1\nTHEIRS\n");
        assert_matches_git(b"1\n2\n3\n", b"1\n2\nOURS\n", b"1\n2");
    }

    #[test]
    fn cjk_and_crlf_match_git() {
        assert_matches_git(
            "你好\n世界\n".as_bytes(),
            "你好\n地球\n".as_bytes(),
            "你好\n人间\n".as_bytes(),
        );
        assert_matches_git(
            "你好\n世界\n".as_bytes(),
            "你好\n地球\n".as_bytes(),
            "你好\n世界\n".as_bytes(),
        );
        assert_matches_git(b"a\r\nb\r\nc\r\n", b"a\r\nB\r\nc\r\n", b"a\r\nb\r\nC\r\n");
        assert_matches_git(b"a\r\nb\r\nc\r\n", b"a\r\nB\r\nc\r\n", b"a\r\nb\r\nc\r\n");
        assert_matches_git(b"a\r\nb\r\nc", b"a\r\nB\r\nc", b"a\r\nb\r\nC");
        assert_matches_git(
            b"\xe4\xbd\xa0\n",
            b"\xe4\xbd\xa0\xe5\xa5\xbd\n",
            b"\xe4\xbd\xa0\n",
        );
    }

    #[test]
    fn binary_is_conflict_with_ours() {
        let base = b"base\0binary".as_slice();
        let ours = b"ours\0binary".as_slice();
        let theirs = b"theirs\0binary".as_slice();
        assert_eq!(
            merge_blobs(Some(base), ours, theirs, &labels(false)).unwrap(),
            Merged::Conflict(ours.to_vec())
        );
        assert_eq!(
            merge_blobs(Some(base), ours, theirs, &labels(true)).unwrap(),
            Merged::Conflict(ours.to_vec())
        );
        // 只有 theirs 是二进制、且没有等值捷径时也要判冲突、保留 ours。
        assert_eq!(
            merge_blobs(Some(b"text\n"), b"ours\n", b"x\0y", &labels(false)).unwrap(),
            Merged::Conflict(b"ours\n".to_vec())
        );
    }

    #[test]
    fn indent_tricky_inputs_match_git() {
        // 与 T6 的 indent/滑动启发式同源的输入。
        assert_matches_git(
            b"\nc\n  x\n  x\n\ty\n\ty\n  x\nb\n",
            b"   }\na\n  x\nb\n  x\nb\n  x\nb\n\ty\nb\n",
            b"\nc\n  x\n  x\n\ty\n\ty\n  x\nb\n",
        );
        assert_matches_git(b"a\n\nb\nc\nd\n", b"a\nb\nc\nd\n\n", b"a\n\nb\nc\nd\n");
        assert_matches_git(b"a\na\na\nb\n", b"a\nb\n", b"a\na\na\nb\n");
        assert_matches_git(b"       }\n", b"   \n}\nx\ny\n", b"       }\n");
    }

    #[test]
    fn random_triples_match_git() {
        const ALPHABET: [&str; 9] = ["a", "b", "", "  x", "\ty", "c", "   }", "d", "中文"];
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |bound: usize| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as usize) % bound
        };
        for _case in 0..220usize {
            let mut base = Vec::new();
            for _ in 0..next(10) {
                base.extend_from_slice(ALPHABET[next(ALPHABET.len())].as_bytes());
                base.push(b'\n');
            }
            let mutate = |src: &[u8], next: &mut dyn FnMut(usize) -> usize| {
                let mut out = src.to_vec();
                for _ in 0..next(3) {
                    let at = next(20);
                    match next(3) {
                        0 => {
                            let insert = ALPHABET[next(ALPHABET.len())].as_bytes();
                            let pos = at.min(out.len());
                            out.splice(pos..pos, insert.iter().copied());
                            if pos == out.len() && !out.is_empty() {
                                out.push(b'\n');
                            }
                        }
                        1 => {
                            if !out.is_empty() {
                                let pos = at.min(out.len() - 1);
                                out.remove(pos);
                            }
                        }
                        _ => {
                            if !out.is_empty() {
                                let pos = at.min(out.len() - 1);
                                out[pos] = b'Z';
                            }
                        }
                    }
                }
                out
            };
            let ours = mutate(&base, &mut next);
            let theirs = mutate(&base, &mut next);
            assert_matches_git(&base, &ours, &theirs);
        }
    }
}
