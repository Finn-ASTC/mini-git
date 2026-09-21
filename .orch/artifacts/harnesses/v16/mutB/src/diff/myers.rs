//! Myers diff 算法。**T6（omp）实现范围**。
//!
//! 输入是两段字节，比较单位是「行」（见 `super::split_lines`，行尾 `\n` 属于该行）。
//! 输出是 `Edit` 序列，且 `Equal`/`Delete`/`Insert` 在两端都连续覆盖全部行。
//!
//! # 为什么是「移植」而不是「重写」
//!
//! 本文件是 git 的 `xdiff`（`xprepare.c` / `xdiffi.c`，v2.47）在「精确行匹配」
//! （无 `-w/-b/--ignore-*` 等空白选项）下的逐函数移植，因为验收标准是
//! **统一 diff 文本与真实 git 逐字节一致**，而 git 的默认输出并不是教科书 Myers：
//!
//! | git 函数 | 本文件 |
//! |---|---|
//! | `xdl_classify_record` | [`classify`]（行 → 等价类 id）|
//! | `xdl_trim_ends` / `xdl_cleanup_records` / `xdl_clean_mmatch` | [`trim_ends`] / [`cleanup`] / [`clean_mmatch`] |
//! | `xdl_recs_cmp` + `xdl_split`（含 `need_min`、启发式与 `mxcost` 兜底） | [`run_myers`] / [`split`] |
//! | `xdl_change_compact` + `group_*` + `measure_split`/`score_*`（缩进启发式，git 2.14+ 默认开启） | [`change_compact`] 及其辅助函数 |
//! | `xdl_build_script` | [`build_edits`] |
//!
//! 差别只在数据结构：git 用 `rchg`/`rindex` 两个数组加偏移，这里用 [`Xdf`] 封装。
//! 「等价类 id」与 git 一样是**行内容的规范键**：本文件里 `class[i] == class[j]`
//! 等价于 `lines[i] == lines[j]`（字节相等），与 git 的 `xdl_recmatch` 在无空白
//! 选项时一致。
//!
//! # 规模保护
//!
//! 没有额外的「保护上限」：git 自带的剪枝（[`cleanup`] 把「在另一侧没有匹配」的
//! 行直接标成变更、排除出 Myers）+ `xdl_split` 的 `mxcost` 兜底已经覆盖了
//! 「20000 行 × 20000 行完全不同」这类输入（剪枝后有效行数为 0，直接秒回）。
//! 唯一与 git 不同的资源策略：递归改成了显式栈（[`run_myers`]），避免极端
//! 偏斜切分下的栈溢出；两者结果相同，因为每个子问题自包含（`xdl_split` 会
//! 先初始化它自己读取的每一个 `kvdf/kvdb` 槽位）。

use std::collections::HashMap;
use std::ops::Range;

use crate::error::Result;

use super::split_lines;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Equal,
    Delete,
    Insert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub op: Op,
    /// 旧文本的行区间（半开）。
    pub old: std::ops::Range<usize>,
    /// 新文本的行区间（半开）。
    pub new: std::ops::Range<usize>,
}

/// 编辑脚本：保证 `Equal` 行在两侧内容一致，且按行序覆盖输入。
///
/// 区间语义：`Equal` 两侧都非空且等长；`Delete` 只覆盖旧文本（`new` 是当前位置的
/// 空区间）；`Insert` 只覆盖新文本（`old` 是当前位置的空区间）。同一个「变更块」
/// 里 `Delete` 一定紧跟在 `Insert` 之前。
pub fn myers(old: &[u8], new: &[u8]) -> Result<Vec<Edit>> {
    let a = split_lines(old);
    let b = split_lines(new);
    let (class_a, class_b, cnt_a, cnt_b) = classify(&a, &b);

    let mut x1 = Xdf::new(a, class_a);
    let mut x2 = Xdf::new(b, class_b);
    trim_ends(&mut x1, &mut x2);
    cleanup(&mut x1, &cnt_b);
    cleanup(&mut x2, &cnt_a);
    run_myers(&mut x1, &mut x2);
    // git 对**两侧**各做一次压缩（`xdl_change_compact(xdf1, xdf2)` 之后是
    // `(xdf2, xdf1)`），所以顺序不能反、也不能只做一次。
    change_compact(&mut x1, &mut x2, true);
    change_compact(&mut x2, &mut x1, true);

    Ok(build_edits(&x1, &x2))
}

// ---------------------------------------------------------------------------
// 常量（与 xdiff 同名同值）
// ---------------------------------------------------------------------------

const MAX_COST_MIN: i64 = 256;
const HEUR_MIN_COST: i64 = 256;
const SNAKE_CNT: i64 = 20;
const K_HEUR: i64 = 4;
const KPDIS_RUN: i64 = 4;
const MAX_EQLIMIT: i64 = 1024;
const SIMSCAN_WINDOW: i64 = 100;
/// `XDL_LINE_MAX`：反向搜索的初始哨兵（C 里是 `LONG_MAX`）。
const LINE_MAX: i64 = i64::MAX;
const MAX_INDENT: i32 = 200;
const MAX_BLANKS: i32 = 20;
const INDENT_HEURISTIC_MAX_SLIDING: i64 = 100;

// `score_add_split` 的经验权重（照抄 xdiffi.c）。
const START_OF_FILE_PENALTY: i32 = 1;
const END_OF_FILE_PENALTY: i32 = 21;
const TOTAL_BLANK_WEIGHT: i32 = -30;
const POST_BLANK_WEIGHT: i32 = 6;
const RELATIVE_INDENT_PENALTY: i32 = -4;
const RELATIVE_INDENT_WITH_BLANK_PENALTY: i32 = 10;
const RELATIVE_OUTDENT_PENALTY: i32 = 24;
const RELATIVE_OUTDENT_WITH_BLANK_PENALTY: i32 = 17;
const RELATIVE_DEDENT_PENALTY: i32 = 23;
const RELATIVE_DEDENT_WITH_BLANK_PENALTY: i32 = 17;
const INDENT_WEIGHT: i32 = 60;

// ---------------------------------------------------------------------------
// 文件状态
// ---------------------------------------------------------------------------

/// 一个文件在 diff 过程中的状态（`xdfile_t` + 分类结果）。
struct Xdf<'a> {
    lines: Vec<&'a [u8]>,
    /// 行等价类 id（对应分类器写回的 `rec->ha`）。
    class: Vec<u32>,
    /// 变更标记。长度为 `nrec + 2`：下标 0 和 `nrec + 1` 是哨兵，
    /// 这样 git 里合法的 `rchg[-1]` / `rchg[nrec]` 访问在 Rust 里也不会越界。
    rchg: Vec<bool>,
    /// 参与 Myers 的「有效行」在 `lines` 中的下标。
    rindex: Vec<usize>,
    /// 有效行的等价类 id。
    ha: Vec<u32>,
    /// 去掉公共前后缀后的区间（`dend` 空文件时为 -1）。
    dstart: i64,
    dend: i64,
    nreff: usize,
}

impl<'a> Xdf<'a> {
    fn new(lines: Vec<&'a [u8]>, class: Vec<u32>) -> Self {
        let nrec = lines.len();
        Xdf {
            lines,
            class,
            rchg: vec![false; nrec + 2],
            rindex: Vec::new(),
            ha: Vec::new(),
            dstart: 0,
            dend: nrec as i64 - 1,
            nreff: 0,
        }
    }

    fn nrec(&self) -> i64 {
        self.lines.len() as i64
    }

    /// `xdf->rchg[i]`，其中 `i` 可以取 -1 与 `nrec`（哨兵区）。
    fn rchg_at(&self, i: i64) -> bool {
        self.rchg[(i + 1) as usize]
    }

    fn set_rchg(&mut self, i: i64, value: bool) {
        self.rchg[(i + 1) as usize] = value;
    }
}

/// 行 → 等价类 id。文件 1 先扫，再扫文件 2（与 git 分类器的分配顺序一致，
/// 不过只影响 id 的编号，不影响任何判定）。
fn classify<'x>(a: &[&'x [u8]], b: &[&'x [u8]]) -> (Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>) {
    let mut ids: HashMap<&'x [u8], u32> = HashMap::new();
    let mut cnt_a: Vec<u32> = Vec::new();
    let mut cnt_b: Vec<u32> = Vec::new();
    let mut class_a = Vec::with_capacity(a.len());
    for line in a {
        let id = *ids.entry(*line).or_insert_with(|| {
            cnt_a.push(0);
            cnt_b.push(0);
            (cnt_a.len() - 1) as u32
        });
        class_a.push(id);
        cnt_a[id as usize] += 1;
    }
    let mut class_b = Vec::with_capacity(b.len());
    for line in b {
        let id = *ids.entry(*line).or_insert_with(|| {
            cnt_a.push(0);
            cnt_b.push(0);
            (cnt_a.len() - 1) as u32
        });
        class_b.push(id);
        cnt_b[id as usize] += 1;
    }
    (class_a, class_b, cnt_a, cnt_b)
}

/// `xdl_trim_ends`：切掉公共前缀与公共后缀。
fn trim_ends(x1: &mut Xdf, x2: &mut Xdf) {
    let (n1, n2) = (x1.lines.len(), x2.lines.len());
    let lim = n1.min(n2);
    let mut prefix = 0;
    while prefix < lim && x1.class[prefix] == x2.class[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < lim - prefix && x1.class[n1 - 1 - suffix] == x2.class[n2 - 1 - suffix] {
        suffix += 1;
    }
    x1.dstart = prefix as i64;
    x2.dstart = prefix as i64;
    x1.dend = n1 as i64 - suffix as i64 - 1;
    x2.dend = n2 as i64 - suffix as i64 - 1;
}

/// `xdl_bogosqrt`：git 的整数平方根近似（返回值是 2 的幂）。
fn bogosqrt(n: i64) -> i64 {
    let mut i = 1i64;
    let mut n = n;
    while n > 0 {
        i <<= 1;
        n >>= 2;
    }
    i
}

/// `xdl_clean_mmatch`：多匹配行是否可以被丢弃（返回 true = 丢弃）。
fn clean_mmatch(dis: &[u8], i: i64, s: i64, e: i64) -> bool {
    let mut s = s;
    let mut e = e;
    if i - s > SIMSCAN_WINDOW {
        s = i - SIMSCAN_WINDOW;
    }
    if e - i > SIMSCAN_WINDOW {
        e = i + SIMSCAN_WINDOW;
    }

    let mut rdis0 = 0i64;
    let mut rpdis0 = 1i64;
    let mut r = 1i64;
    while i - r >= s {
        match dis[(i - r) as usize] {
            0 => rdis0 += 1,
            2 => rpdis0 += 1,
            _ => break,
        }
        r += 1;
    }
    if rdis0 == 0 {
        return false;
    }

    let mut rdis1 = 0i64;
    let mut rpdis1 = 1i64;
    let mut r = 1i64;
    while i + r <= e {
        match dis[(i + r) as usize] {
            0 => rdis1 += 1,
            2 => rpdis1 += 1,
            _ => break,
        }
        r += 1;
    }
    if rdis1 == 0 {
        return false;
    }
    rdis1 += rdis0;
    rpdis1 += rpdis0;
    rpdis1 * KPDIS_RUN < (rpdis1 + rdis1)
}

/// `xdl_cleanup_records`：把「另一侧没有匹配」的行直接标记为变更（剪枝），
/// 其余多匹配行按 `clean_mmatch` 决定去留。
fn cleanup(x: &mut Xdf, other_counts: &[u32]) {
    let nrec = x.nrec();
    let mut dis = vec![0u8; nrec as usize + 1];
    let mlim = bogosqrt(nrec).min(MAX_EQLIMIT);

    let mut i = x.dstart;
    while i <= x.dend {
        let nm = other_counts[x.class[i as usize] as usize] as i64;
        dis[i as usize] = if nm == 0 {
            0
        } else if nm >= mlim {
            2
        } else {
            1
        };
        i += 1;
    }

    x.rindex.clear();
    x.ha.clear();
    let (dstart, dend) = (x.dstart, x.dend);
    let mut i = dstart;
    while i <= dend {
        let d = dis[i as usize];
        let keep = d == 1 || (d == 2 && !clean_mmatch(&dis, i, dstart, dend));
        if keep {
            x.rindex.push(i as usize);
            x.ha.push(x.class[i as usize]);
        } else {
            x.set_rchg(i, true);
        }
        i += 1;
    }
    x.nreff = x.rindex.len();
}

// ---------------------------------------------------------------------------
// Myers 分割（xdl_recs_cmp / xdl_split）
// ---------------------------------------------------------------------------

/// `kvdf` / `kvdb`：正反两个方向的「最远到达点」数组。
/// git 用 `kvdf += nreff2 + 1` 做基准偏移；下标 d 就是对角线号 `i1 - i2`。
struct Diags {
    fwd: Vec<i64>,
    bwd: Vec<i64>,
    base: i64,
}

impl Diags {
    fn new(nreff1: usize, nreff2: usize) -> Self {
        let size = nreff1 + nreff2 + 3;
        Diags {
            fwd: vec![0; size],
            bwd: vec![0; size],
            base: nreff2 as i64 + 1,
        }
    }

    fn idx(&self, d: i64) -> usize {
        (d + self.base) as usize
    }
}

struct Env {
    mxcost: i64,
    snake_cnt: i64,
    heur_min: i64,
}

struct Split {
    i1: i64,
    i2: i64,
    min_lo: bool,
    min_hi: bool,
}

/// 一个待处理的「盒子」（git 用递归，这里用显式栈；同一个结构也用作
/// [`split`] 的参数，省掉一长串 `off/lim` 参数）。
#[derive(Clone, Copy)]
struct Region {
    off1: i64,
    lim1: i64,
    off2: i64,
    lim2: i64,
    need_min: bool,
}

/// `xdl_split`：在「盒子」里同时从两个方向找中线（middle snake）。
fn split(region: &Region, diags: &mut Diags, ha1: &[u32], ha2: &[u32], env: &Env) -> Split {
    let Region {
        off1,
        lim1,
        off2,
        lim2,
        need_min,
    } = *region;
    let dmin = off1 - lim2;
    let dmax = lim1 - off2;
    let fmid = off1 - off2;
    let bmid = lim1 - lim2;
    let odd = (fmid - bmid) & 1 != 0;

    let (mut fmin, mut fmax) = (fmid, fmid);
    let (mut bmin, mut bmax) = (bmid, bmid);

    let fmid_idx = diags.idx(fmid);
    diags.fwd[fmid_idx] = off1;
    let bmid_idx = diags.idx(bmid);
    diags.bwd[bmid_idx] = lim1;

    let mut ec = 1i64;
    loop {
        let mut got_snake = false;

        // ---- 正向扩展一格对角线域，并给域外的槽位放哨兵 ----
        if fmin > dmin {
            fmin -= 1;
            let idx = diags.idx(fmin - 1);
            diags.fwd[idx] = -1;
        } else {
            fmin += 1;
        }
        if fmax < dmax {
            fmax += 1;
            let idx = diags.idx(fmax + 1);
            diags.fwd[idx] = -1;
        } else {
            fmax -= 1;
        }

        let mut d = fmax;
        while d >= fmin {
            let mut i1 = if diags.fwd[diags.idx(d - 1)] >= diags.fwd[diags.idx(d + 1)] {
                diags.fwd[diags.idx(d - 1)] + 1
            } else {
                diags.fwd[diags.idx(d + 1)]
            };
            let prev1 = i1;
            let mut i2 = i1 - d;
            while i1 < lim1 && i2 < lim2 && ha1[i1 as usize] == ha2[i2 as usize] {
                i1 += 1;
                i2 += 1;
            }
            if i1 - prev1 > env.snake_cnt {
                got_snake = true;
            }
            let idx = diags.idx(d);
            diags.fwd[idx] = i1;
            if odd && bmin <= d && d <= bmax && diags.bwd[diags.idx(d)] <= i1 {
                return Split {
                    i1,
                    i2,
                    min_lo: true,
                    min_hi: true,
                };
            }
            d -= 2;
        }

        // ---- 反向扩展 ----
        if bmin > dmin {
            bmin -= 1;
            let idx = diags.idx(bmin - 1);
            diags.bwd[idx] = LINE_MAX;
        } else {
            bmin += 1;
        }
        if bmax < dmax {
            bmax += 1;
            let idx = diags.idx(bmax + 1);
            diags.bwd[idx] = LINE_MAX;
        } else {
            bmax -= 1;
        }

        let mut d = bmax;
        while d >= bmin {
            let mut i1 = if diags.bwd[diags.idx(d - 1)] < diags.bwd[diags.idx(d + 1)] {
                diags.bwd[diags.idx(d - 1)]
            } else {
                diags.bwd[diags.idx(d + 1)] - 1
            };
            let prev1 = i1;
            let mut i2 = i1 - d;
            while i1 > off1 && i2 > off2 && ha1[(i1 - 1) as usize] == ha2[(i2 - 1) as usize] {
                i1 -= 1;
                i2 -= 1;
            }
            if prev1 - i1 > env.snake_cnt {
                got_snake = true;
            }
            let idx = diags.idx(d);
            diags.bwd[idx] = i1;
            if !odd && fmin <= d && d <= fmax && i1 <= diags.fwd[diags.idx(d)] {
                return Split {
                    i1,
                    i2,
                    min_lo: true,
                    min_hi: true,
                };
            }
            d -= 2;
        }

        if !need_min && got_snake && ec > env.heur_min {
            // 正向采样：找「离角落足够远且带蛇形」的对角线。
            let mut best = 0i64;
            let mut best_split = None;
            let mut d = fmax;
            while d >= fmin {
                let dd = if d > fmid { d - fmid } else { fmid - d };
                let i1 = diags.fwd[diags.idx(d)];
                let i2 = i1 - d;
                let v = (i1 - off1) + (i2 - off2) - dd;
                if v > K_HEUR * ec
                    && v > best
                    && off1 + env.snake_cnt <= i1
                    && i1 < lim1
                    && off2 + env.snake_cnt <= i2
                    && i2 < lim2
                {
                    let mut k = 1i64;
                    while ha1[(i1 - k) as usize] == ha2[(i2 - k) as usize] {
                        if k == env.snake_cnt {
                            best = v;
                            best_split = Some(Split {
                                i1,
                                i2,
                                min_lo: true,
                                min_hi: false,
                            });
                            break;
                        }
                        k += 1;
                    }
                }
                d -= 2;
            }
            if best > 0 {
                return best_split.expect("best 与 best_split 同步设置");
            }

            // 反向采样。
            let mut best = 0i64;
            let mut best_split = None;
            let mut d = bmax;
            while d >= bmin {
                let dd = if d > bmid { d - bmid } else { bmid - d };
                let i1 = diags.bwd[diags.idx(d)];
                let i2 = i1 - d;
                let v = (lim1 - i1) + (lim2 - i2) - dd;
                if v > K_HEUR * ec
                    && v > best
                    && off1 < i1
                    && i1 <= lim1 - env.snake_cnt
                    && off2 < i2
                    && i2 <= lim2 - env.snake_cnt
                {
                    let mut k = 0i64;
                    while ha1[(i1 + k) as usize] == ha2[(i2 + k) as usize] {
                        if k == env.snake_cnt - 1 {
                            best = v;
                            best_split = Some(Split {
                                i1,
                                i2,
                                min_lo: false,
                                min_hi: true,
                            });
                            break;
                        }
                        k += 1;
                    }
                }
                d -= 2;
            }
            if best > 0 {
                return best_split.expect("best 与 best_split 同步设置");
            }
        }

        // 花费超上限：用 (i1 + i2) 度量挑一个「最远」的点收工（与 git 一样是
        // 次优解，但保证复杂输入不会失控）。
        if ec >= env.mxcost {
            let mut fbest = -1i64;
            let mut fbest1 = -1i64;
            let mut d = fmax;
            while d >= fmin {
                let mut i1 = diags.fwd[diags.idx(d)].min(lim1);
                let mut i2 = i1 - d;
                if lim2 < i2 {
                    i1 = lim2 + d;
                    i2 = lim2;
                }
                if fbest < i1.saturating_add(i2) {
                    fbest = i1 + i2;
                    fbest1 = i1;
                }
                d -= 2;
            }

            let mut bbest = LINE_MAX;
            let mut bbest1 = LINE_MAX;
            let mut d = bmax;
            while d >= bmin {
                let mut i1 = diags.bwd[diags.idx(d)].max(off1);
                let mut i2 = i1 - d;
                if i2 < off2 {
                    i1 = off2 + d;
                    i2 = off2;
                }
                if i1.saturating_add(i2) < bbest {
                    bbest = i1 + i2;
                    bbest1 = i1;
                }
                d -= 2;
            }

            if (lim1 + lim2) - bbest < fbest - (off1 + off2) {
                return Split {
                    i1: fbest1,
                    i2: fbest - fbest1,
                    min_lo: true,
                    min_hi: false,
                };
            }
            return Split {
                i1: bbest1,
                i2: bbest - bbest1,
                min_lo: false,
                min_hi: true,
            };
        }

        ec += 1;
    }
}

/// `xdl_recs_cmp`：分治地把变更行打上 `rchg` 标记。
fn run_myers(x1: &mut Xdf, x2: &mut Xdf) {
    let ndiags = (x1.nreff + x2.nreff) as i64 + 3;
    let env = Env {
        mxcost: bogosqrt(ndiags).max(MAX_COST_MIN),
        snake_cnt: SNAKE_CNT,
        heur_min: HEUR_MIN_COST,
    };

    let ha1 = &x1.ha;
    let ha2 = &x2.ha;
    let rindex1 = &x1.rindex;
    let rindex2 = &x2.rindex;
    let rchg1 = &mut x1.rchg;
    let rchg2 = &mut x2.rchg;
    let mut diags = Diags::new(x1.nreff, x2.nreff);

    let mut stack = vec![Region {
        off1: 0,
        lim1: x1.nreff as i64,
        off2: 0,
        lim2: x2.nreff as i64,
        need_min: false,
    }];

    while let Some(mut frame) = stack.pop() {
        // 先吃掉盒子两端的蛇形（公共部分）。
        while frame.off1 < frame.lim1
            && frame.off2 < frame.lim2
            && ha1[frame.off1 as usize] == ha2[frame.off2 as usize]
        {
            frame.off1 += 1;
            frame.off2 += 1;
        }
        while frame.off1 < frame.lim1
            && frame.off2 < frame.lim2
            && ha1[(frame.lim1 - 1) as usize] == ha2[(frame.lim2 - 1) as usize]
        {
            frame.lim1 -= 1;
            frame.lim2 -= 1;
        }

        if frame.off1 == frame.lim1 {
            for i in frame.off2..frame.lim2 {
                rchg2[rindex2[i as usize] + 1] = true;
            }
        } else if frame.off2 == frame.lim2 {
            for i in frame.off1..frame.lim1 {
                rchg1[rindex1[i as usize] + 1] = true;
            }
        } else {
            let spl = split(&frame, &mut diags, ha1, ha2, &env);
            stack.push(Region {
                off1: frame.off1,
                lim1: spl.i1,
                off2: frame.off2,
                lim2: spl.i2,
                need_min: spl.min_lo,
            });
            stack.push(Region {
                off1: spl.i1,
                lim1: frame.lim1,
                off2: spl.i2,
                lim2: frame.lim2,
                need_min: spl.min_hi,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// 变更块压缩（xdl_change_compact）
// ---------------------------------------------------------------------------

/// 一个（可能为空的）变更块：`[start, end)`。
#[derive(Debug, Clone, Copy)]
struct Group {
    start: i64,
    end: i64,
}

fn group_init(x: &Xdf) -> Group {
    let mut g = Group { start: 0, end: 0 };
    while x.rchg_at(g.end) {
        g.end += 1;
    }
    g
}

/// 返回 true = 成功移动到下一个（可能为空的）块。
fn group_next(x: &Xdf, g: &mut Group) -> bool {
    if g.end == x.nrec() {
        return false;
    }
    g.start = g.end + 1;
    g.end = g.start;
    while x.rchg_at(g.end) {
        g.end += 1;
    }
    true
}

fn group_previous(x: &Xdf, g: &mut Group) -> bool {
    if g.start == 0 {
        return false;
    }
    g.end = g.start - 1;
    g.start = g.end;
    while g.start > 0 && x.rchg_at(g.start - 1) {
        g.start -= 1;
    }
    true
}

/// 把块整体往下滑一格（如果允许），撞上后面的块就吞并。
fn group_slide_down(x: &mut Xdf, g: &mut Group) -> bool {
    if g.end < x.nrec() && x.class[g.start as usize] == x.class[g.end as usize] {
        x.set_rchg(g.start, false);
        g.start += 1;
        x.set_rchg(g.end, true);
        g.end += 1;
        while x.rchg_at(g.end) {
            g.end += 1;
        }
        true
    } else {
        false
    }
}

/// 把块整体往上滑一格（如果允许），撞上前面的块就吞并。
fn group_slide_up(x: &mut Xdf, g: &mut Group) -> bool {
    if g.start > 0 && x.class[(g.start - 1) as usize] == x.class[(g.end - 1) as usize] {
        g.start -= 1;
        x.set_rchg(g.start, true);
        g.end -= 1;
        x.set_rchg(g.end, false);
        while g.start > 0 && x.rchg_at(g.start - 1) {
            g.start -= 1;
        }
        true
    } else {
        false
    }
}

/// `get_indent`：空白行返回 -1，否则返回缩进列数（TAB = 8，`MAX_INDENT` 封顶）。
fn get_indent(line: &[u8]) -> i32 {
    let mut ret = 0i32;
    for &c in line {
        if !is_space(c) {
            return ret;
        } else if c == b' ' {
            ret += 1;
        } else if c == b'\t' {
            ret += 8 - ret % 8;
        }
        if ret >= MAX_INDENT {
            return MAX_INDENT;
        }
    }
    -1
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

#[derive(Debug, Clone, Copy, Default)]
struct SplitMeasurement {
    end_of_file: bool,
    indent: i32,
    pre_blank: i32,
    pre_indent: i32,
    post_blank: i32,
    post_indent: i32,
}

/// `measure_split`：统计「在第 split 行之前切开」时周围的缩进/空行情况。
fn measure_split(x: &Xdf, split: i64) -> SplitMeasurement {
    let nrec = x.nrec();
    let mut m = SplitMeasurement::default();
    if split >= nrec {
        m.end_of_file = true;
        m.indent = -1;
    } else {
        m.end_of_file = false;
        m.indent = get_indent(x.lines[split as usize]);
    }

    m.pre_blank = 0;
    m.pre_indent = -1;
    let mut i = split - 1;
    while i >= 0 {
        m.pre_indent = get_indent(x.lines[i as usize]);
        if m.pre_indent != -1 {
            break;
        }
        m.pre_blank += 1;
        if m.pre_blank == MAX_BLANKS {
            m.pre_indent = 0;
            break;
        }
        i -= 1;
    }

    m.post_blank = 0;
    m.post_indent = -1;
    let mut i = split + 1;
    while i < nrec {
        m.post_indent = get_indent(x.lines[i as usize]);
        if m.post_indent != -1 {
            break;
        }
        m.post_blank += 1;
        if m.post_blank == MAX_BLANKS {
            m.post_indent = 0;
            break;
        }
        i += 1;
    }

    m
}

#[derive(Debug, Clone, Copy, Default)]
struct Score {
    effective_indent: i32,
    penalty: i32,
}

fn score_add_split(m: &SplitMeasurement, s: &mut Score) {
    if m.pre_indent == -1 && m.pre_blank == 0 {
        s.penalty += START_OF_FILE_PENALTY;
    }
    if m.end_of_file {
        s.penalty += END_OF_FILE_PENALTY;
    }

    let post_blank = if m.indent == -1 { 1 + m.post_blank } else { 0 };
    let total_blank = m.pre_blank + post_blank;

    s.penalty += TOTAL_BLANK_WEIGHT * total_blank;
    s.penalty += POST_BLANK_WEIGHT * post_blank;

    let indent = if m.indent != -1 {
        m.indent
    } else {
        m.post_indent
    };
    let any_blanks = total_blank != 0;

    s.effective_indent += indent;

    if indent == -1 || m.pre_indent == -1 {
        // 无需额外调整。
    } else if indent > m.pre_indent {
        s.penalty += if any_blanks {
            RELATIVE_INDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_INDENT_PENALTY
        };
    } else if indent < m.pre_indent {
        if m.post_indent != -1 && m.post_indent > indent {
            s.penalty += if any_blanks {
                RELATIVE_OUTDENT_WITH_BLANK_PENALTY
            } else {
                RELATIVE_OUTDENT_PENALTY
            };
        } else {
            s.penalty += if any_blanks {
                RELATIVE_DEDENT_WITH_BLANK_PENALTY
            } else {
                RELATIVE_DEDENT_PENALTY
            };
        }
    }
}

fn score_cmp(s1: &Score, s2: &Score) -> i32 {
    let cmp_indents = (s1.effective_indent > s2.effective_indent) as i32
        - (s1.effective_indent < s2.effective_indent) as i32;
    INDENT_WEIGHT * cmp_indents + (s1.penalty - s2.penalty)
}

/// `xdl_change_compact`：在「相邻行内容相同」的自由度内把变更块挪到更好看的位置。
/// `indent = true` 表示启用缩进启发式（git 2.14+ 的默认）。
fn change_compact(x: &mut Xdf, xo: &mut Xdf, indent: bool) {
    let mut g = group_init(x);
    let mut go = group_init(xo);

    loop {
        if g.end != g.start {
            let mut end_matching_other;
            let mut earliest_end;

            loop {
                let groupsize = g.end - g.start;
                end_matching_other = -1;

                // 先尽量往上滑。
                while group_slide_up(x, &mut g) {
                    debug_assert!(group_previous(xo, &mut go));
                }
                earliest_end = g.end;
                if go.end > go.start {
                    end_matching_other = g.end;
                }

                // 再尽量往下滑。
                loop {
                    if !group_slide_down(x, &mut g) {
                        break;
                    }
                    debug_assert!(group_next(xo, &mut go));
                    if go.end > go.start {
                        end_matching_other = g.end;
                    }
                }

                if groupsize == g.end - g.start {
                    break;
                }
            }

            if g.end == earliest_end {
                // 滑不动，什么都不用做。
            } else if end_matching_other != -1 {
                // 有可以对齐的对方变更块：退回最后一个能对齐的位置。
                while go.end == go.start {
                    debug_assert!(group_slide_up(x, &mut g));
                    debug_assert!(group_previous(xo, &mut go));
                }
            } else if indent {
                let mut best_shift = -1i64;
                let mut best_score = Score::default();

                let mut shift = earliest_end;
                let groupsize = g.end - g.start;
                if g.end - groupsize - 1 > shift {
                    shift = g.end - groupsize - 1;
                }
                if g.end - INDENT_HEURISTIC_MAX_SLIDING > shift {
                    shift = g.end - INDENT_HEURISTIC_MAX_SLIDING;
                }
                while shift <= g.end {
                    let mut score = Score::default();
                    score_add_split(&measure_split(x, shift), &mut score);
                    score_add_split(&measure_split(x, shift - groupsize), &mut score);
                    if best_shift == -1 || score_cmp(&score, &best_score) <= 0 {
                        best_score = score;
                        best_shift = shift;
                    }
                    shift += 1;
                }

                while g.end > best_shift {
                    debug_assert!(group_slide_up(x, &mut g));
                    debug_assert!(group_previous(xo, &mut go));
                }
            }
        }

        if !group_next(x, &mut g) {
            break;
        }
        debug_assert!(group_next(xo, &mut go));
    }

    debug_assert!(!group_next(xo, &mut go));
}

// ---------------------------------------------------------------------------
// 变更块 → 编辑脚本（xdl_build_script）
// ---------------------------------------------------------------------------

/// 一个变更块：旧文本 `old_start..old_start + old_len`，新文本 `new_start..+new_len`。
struct Atom {
    old_start: usize,
    old_len: usize,
    new_start: usize,
    new_len: usize,
}

fn build_atoms(x1: &Xdf, x2: &Xdf) -> Vec<Atom> {
    let mut atoms = Vec::new();
    let (mut i1, mut i2) = (x1.nrec(), x2.nrec());
    while i1 >= 0 || i2 >= 0 {
        let marked1 = i1 >= 0 && x1.rchg_at(i1 - 1);
        let marked2 = i2 >= 0 && x2.rchg_at(i2 - 1);
        if marked1 || marked2 {
            let end1 = i1;
            let end2 = i2;
            while i1 >= 0 && x1.rchg_at(i1 - 1) {
                i1 -= 1;
            }
            while i2 >= 0 && x2.rchg_at(i2 - 1) {
                i2 -= 1;
            }
            atoms.push(Atom {
                old_start: i1 as usize,
                old_len: (end1 - i1) as usize,
                new_start: i2 as usize,
                new_len: (end2 - i2) as usize,
            });
        }
        i1 -= 1;
        i2 -= 1;
    }
    // git 用链表头插，最终就是升序；这里用 push + reverse 得到同样的顺序。
    atoms.reverse();
    atoms
}

fn build_edits(x1: &Xdf, x2: &Xdf) -> Vec<Edit> {
    let atoms = build_atoms(x1, x2);
    let mut edits = Vec::new();
    let (mut o, mut n) = (0usize, 0usize);

    for atom in &atoms {
        if atom.old_start > o || atom.new_start > n {
            debug_assert_eq!(
                atom.old_start - o,
                atom.new_start - n,
                "未变更区间在两侧必须等长"
            );
            debug_assert_eq!(
                x1.lines[o..atom.old_start],
                x2.lines[n..atom.new_start],
                "未变更区间在两侧内容必须一致"
            );
            edits.push(Edit {
                op: Op::Equal,
                old: o..atom.old_start,
                new: n..atom.new_start,
            });
        }
        if atom.old_len > 0 {
            let end = atom.old_start + atom.old_len;
            edits.push(Edit {
                op: Op::Delete,
                old: atom.old_start..end,
                new: atom.new_start..atom.new_start,
            });
        }
        if atom.new_len > 0 {
            let end = atom.new_start + atom.new_len;
            edits.push(Edit {
                op: Op::Insert,
                old: atom.old_start + atom.old_len..atom.old_start + atom.old_len,
                new: atom.new_start..end,
            });
        }
        o = atom.old_start + atom.old_len;
        n = atom.new_start + atom.new_len;
    }

    if o < x1.lines.len() || n < x2.lines.len() {
        debug_assert_eq!(x1.lines.len() - o, x2.lines.len() - n);
        debug_assert_eq!(x1.lines[o..], x2.lines[n..]);
        edits.push(Edit {
            op: Op::Equal,
            old: o..x1.lines.len(),
            new: n..x2.lines.len(),
        });
    }

    edits
}

/// 便于 `unified` 复用：把编辑脚本归并成变更块（每个块 = 紧邻的 Delete + Insert）。
pub(super) fn change_atoms(edits: &[Edit]) -> Vec<(Range<usize>, Range<usize>)> {
    let mut atoms: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    let mut cur: Option<(Range<usize>, Range<usize>)> = None;
    for edit in edits {
        if edit.op == Op::Equal {
            if let Some(atom) = cur.take() {
                atoms.push(atom);
            }
            continue;
        }
        match cur.as_mut() {
            None => cur = Some((edit.old.clone(), edit.new.clone())),
            Some((old, new)) => {
                old.end = old.end.max(edit.old.end);
                new.end = new.end.max(edit.new.end);
            }
        }
    }
    if let Some(atom) = cur {
        atoms.push(atom);
    }
    atoms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::split_lines;

    /// 编辑脚本的契约：按行序完整覆盖两侧、区间首尾相接、`Equal` 两侧内容一致。
    fn assert_valid_script(old: &[u8], new: &[u8]) {
        let edits = myers(old, new).expect("myers failed");
        let old_lines = split_lines(old);
        let new_lines = split_lines(new);
        let (mut o, mut n) = (0usize, 0usize);
        for edit in &edits {
            assert_eq!(edit.old.start, o, "旧侧区间必须首尾相接: {edits:?}");
            assert_eq!(edit.new.start, n, "新侧区间必须首尾相接: {edits:?}");
            match edit.op {
                Op::Equal => {
                    assert_eq!(
                        edit.old.end - edit.old.start,
                        edit.new.end - edit.new.start,
                        "Equal 两侧必须等长: {edits:?}"
                    );
                    assert_eq!(
                        old_lines[edit.old.clone()],
                        new_lines[edit.new.clone()],
                        "Equal 两侧内容必须一致: {edits:?}"
                    );
                }
                Op::Delete => {
                    assert!(edit.old.end > edit.old.start, "Delete 必须有旧侧区间");
                    assert_eq!(edit.new.end, edit.new.start, "Delete 不覆盖新侧");
                }
                Op::Insert => {
                    assert!(edit.new.end > edit.new.start, "Insert 必须有新侧区间");
                    assert_eq!(edit.old.end, edit.old.start, "Insert 不覆盖旧侧");
                }
            }
            o = edit.old.end;
            n = edit.new.end;
        }
        assert_eq!(o, old_lines.len(), "必须覆盖旧文件全部行: {edits:?}");
        assert_eq!(n, new_lines.len(), "必须覆盖新文件全部行: {edits:?}");
    }

    #[test]
    fn identical_inputs_are_one_equal_span() {
        assert_eq!(
            myers(b"a\nb\n", b"a\nb\n").unwrap(),
            vec![Edit {
                op: Op::Equal,
                old: 0..2,
                new: 0..2,
            }]
        );
        assert_eq!(myers(b"", b"").unwrap(), Vec::new());
    }

    #[test]
    fn scripts_cover_both_sides_in_order() {
        let cases: [(&[u8], &[u8]); 8] = [
            (b"", b"a\nb\n"),
            (b"a\nb\n", b""),
            (b"a\nb\nc\n", b"a\nc\n"),
            (b"a\nc\n", b"a\nb\nc\n"),
            (b"a\nb\nc\n", b"a\nX\nc\n"),
            (b"a\nb", b"a\nb\n"),
            (b"a\nb\n", b"a\nb"),
            (b"a\na\na\nb\n", b"a\nb\n"),
        ];
        for (old, new) in cases {
            assert_valid_script(old, new);
        }
    }

    #[test]
    fn random_scripts_are_valid() {
        const ALPHABET: [&str; 6] = ["a", "b", "", "  c", "d", "e"];
        let mut state = 0x0123_4567_89ab_cdefu64;
        let mut next = |bound: usize| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as usize) % bound
        };
        for case in 0..200usize {
            let mut old = Vec::new();
            for _ in 0..next(20) {
                old.extend_from_slice(ALPHABET[next(ALPHABET.len())].as_bytes());
                old.push(b'\n');
            }
            let mut new = Vec::new();
            for _ in 0..next(20) {
                new.extend_from_slice(ALPHABET[next(ALPHABET.len())].as_bytes());
                new.push(b'\n');
            }
            if case % 5 == 0 && !old.is_empty() {
                old.pop(); // 制造「末行无换行」的输入
            }
            assert_valid_script(&old, &new);
        }
    }

    #[test]
    fn large_unrelated_inputs_are_pruned_and_fast() {
        // 20000 × 20000 行、完全没有公共行：git 的剪枝应当让它在毫秒级返回，
        // 而不是 O(N*M) 的内存/时间爆炸。
        let mut old = Vec::new();
        for i in 0..20000 {
            old.extend_from_slice(format!("old-{i}\n").as_bytes());
        }
        let mut new = Vec::new();
        for i in 0..20000 {
            new.extend_from_slice(format!("new-{i}\n").as_bytes());
        }
        let edits = myers(&old, &new).unwrap();
        assert_eq!(
            edits,
            vec![
                Edit {
                    op: Op::Delete,
                    old: 0..20000,
                    new: 0..0,
                },
                Edit {
                    op: Op::Insert,
                    old: 20000..20000,
                    new: 0..20000,
                },
            ]
        );
    }
}
