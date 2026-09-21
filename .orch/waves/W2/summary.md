# Wave W2 复盘（T5–T8：4 个 kind 并行 + 交叉验证 + 1 次返工）

- 时间：2026-09-19 20:13:41 → 20:49:13 +08:00（wall **35m32s**）
- 任务数：**4 个实现任务 + 4 个交叉验证任务 + 1 个返工轮（T6b）+ 1 个复验轮（V6b）**
- 参与 kind：**hermes / omp / codex / opencode**（4 个 kind 全部上场，且每个任务的作者 kind ≠ 验证 kind）
- 环境：herdr **用户既存 `default` 会话**（可见模式），每 agent 一个可见 workspace（`--no-focus`），
  工作区策略 A（共享 checkout、目录互斥）
- 运行模型：**W2 是「先作者后验证」两段式** —— 四个作者同时开工，交付后各起一个**独立 space** 的验证者

| 任务 | 写作用域 | 作者 | 验证者 |
|---|---|---|---|
| T5 odb loose + `mg cat-file` | `src/odb/loose.rs`、`src/cli/cat_file.rs` | **hermes** | **opencode** |
| T6 diff 引擎 + `mg diff` | `src/diff/{myers,unified}.rs`、`src/cli/diff.rs` | **omp** | **codex** |
| T7 merge-base | `src/merge/merge_base.rs` | **codex** | **omp** |
| T8 pkt-line | `src/transport/pktline.rs` | **opencode** | **hermes** |

## 1. 逐任务结果

| 轮次 | agent / pane | 提交 → 结果 | 用时 | 首位判定 | 备注 |
|---|---|---|---|---|---|
| T5 | `w2-t5-hermes` / `wV:p1` | 20:13:41 → 20:25:38 | 11m57s | — | 主动上报 2 件事：任务包 tree mode 规格自相矛盾（W2-D2）、自己误 export `GIT_CONFIG_*`（W2-D4） |
| T6 | `w2-t6-omp` / `wW:p1` | 20:13:41 → 20:26:40 | 12m59s | — | 交付含「教科书 Myers 不够、改成 git xdiff 移植」的证据链；rename 检测列为非目标 |
| T7 | `w2-t7-codex` / `wX:p1` | 20:13:41 → 20:30:00 | 16m19s | — | 作者自曝：初版测试用单调时间戳导致变异体逃逸，改成 skew/同戳后检出（W2-D3） |
| T8 | `w2-t8-opencode` / `wY:p1` | 20:13:41 → 20:15:49 | **2m08s** | — | 用真实 `upload-pack --advertise-refs` 逐帧往返对拍 |
| V8 | `w2-v8-hermes` / `wZ:p1` | 20:16:29 → 20:23:47 | 7m18s | **PASS** | 22 份证据文件；6 个变异体全部被检出（其中 1 个连作者自带测试都漏检）；**更正了任务书的错误真值来源** |
| V5 | `w2-v5-opencode` / `w0:p1` | 20:30:05 → 20:33:19 | ~3m14s | **PASS** | 6 个独立用例；作者 6 个单测被独立复跑 |
| V7 | `w2-v7-omp` / `w12:p1` | 20:30:55 → 20:35:30 | 4m35s | **PASS** | 10 个独立用例；2 个变异体 10/10 与 6/10 被检出 |
| V6 | `w2-v6-codex` / `w11:p1` | 20:30:55 → 20:37:46 | 6m51s | **FAIL（窄口径）** | 抓到 `mg diff` 在含空格路径上的 TAB 缺失（W2-D1），并留下 `#[ignore]` 回归用例 |
| **T6b（返工）** | `w2-t6b-omp` / `w13:p1` | 20:40:10 → 20:43:20 | 3m10s | — | 只改 `src/diff/unified.rs`；未改 `src/cli/diff.rs`（并说明了理由） |
| **V6b（复验）** | `w2-v6b-codex` / `w14:p1` | 20:44:30 → 20:49:13 | 4m43s | **PASS** | 起手先做白名单审计（确认只改了 `unified.rs`）；52 次 CLI 逐字节对拍 + 28 次引擎对拍；两个方向的变异体（从不补/总是补）都被抓 |

首轮通过率：**3/4 = 75%**（T5/T7/T8 首轮 PASS；T6 被验证者判 FAIL，走了一次返工 + 复验）。

## 2. 核心指标（§6）

| 指标 | 数值 | 说明/证据 |
|---|---|---|
| 协议有效率 | **10/10 = 100%** | 每一轮 `protocol.py validate --check-files` 都 exit 0；`job_id`/`round_id`/result 路径零错配 |
| **假绿率** | **0** | 4 个作者声称的测试数量与结论，独立复跑全部成立；V6 的 FAIL 不是「作者撒谎」，而是**测试矩阵盲区**（作者的 soak 覆盖了 900 次 CLI 对拍，但没造过含空格的路径） |
| **越界率** | **0 个文件** | 四个验证者各自用 `baseline.txt` + mtime 窗口审计；作者改动全部落在白名单内；`Cargo.toml`/`Cargo.lock`/所有 `mod.rs` 未变 |
| 接口漂移次数 | **0** | `scripts/check-freeze.sh` → `checked 21 file(s), drift 0`（controller 复跑） |
| blocked 处理 | 协议级 blocked **0**；原生对话框 **6 次** | codex 侧：T7 作者 2 次、V6 4 次（3 次半自动 + 1 次人工）；omp/hermes/opencode 0 次 |
| 交叉验证真实率 | **5/5 真实执行** | 5 个验证者都新建了自己的 test target（`verify_odb` 6 例、`verify_diff` 16+1 例、`verify_mergebase` 10 例、`verify_pktline` 8 例、`verify_diff_paths`），真值零硬编码 |
| 并行加速比 | **作者段 2.66×**；含验证全流程 2.06× | Σ作者耗时 2603s（T8 128s + T5 717s + T6 779s + T7 979s）/ 作者段 wall 979s（20:13:41→20:30:00）= 2.66×；含 5 个验证轮 + 返工轮的 Σ=4394s / 全 wave 2132s = 2.06× |
| 返工轮次占比 | **1/10 = 10%** | T6 → T6b → V6b（返工 + 复验共 7m53s，占全 wave 22%）|
| token 用量（原生计数） | 见 §5 | 取自各 TUI 状态行 |

## 3. 验证者的独立发现（本轮最大价值）

1. **W2-D1（真缺陷）**：`mg diff` 的 `--- `/`+++ ` 行在路径含空格时缺 git 的 TAB 填充。
   V6 用「平行仓库 + `cmp` 逐字节」抓到，并**主动**把它写成 `#[ignore]` 的回归用例，
   避免把共享 checkout 的 `cargo test --offline` 弄红（共享环境纪律）。
2. **V8 更正任务书**：任务书写「`git ls-remote <path>` 的 stdout 是原始 pkt-line」，实测是纯文本
   `oid\tref` 行；V8 改用 `git upload-pack --stateless-rpc --advertise-refs`，
   并补了 `git http-backend` + v2 作为第二数据源。→ 若验证者照抄任务书，就会产出**自洽的假绿**。
3. **V7 复现了 controller 的提醒**：把 DAG 时间戳写成与拓扑相反、criss-cross 同戳之后，
   「只按时间戳取最新祖先」的变异体才被检出（作者 T7 自己也踩过同一个坑）。
4. **V5/V6/V7 都主动说明了「按 controller 备注不判 FAIL」的边界**（rename、多个 merge-base、tree mode 位数），
   没有把非目标当成缺陷刷分。

## 4. 过程观察（编排 skill 的真实行为）

- **codex 审批仍是最大成本**：V6 在 6m51s 内弹了 4 次原生审批；本轮 controller 首次启用
  `.orch/artifacts/tools/orch-autogrant.py`（只对「写在 `/tmp`、只读仓库」的命令按 1，逐条记 `.orch/artifacts/logs/orch-approvals.log`，
  命中 `rm -rf /`、`sudo`、`git reset/checkout/clean`、写 `src/` 等模式则停机）→ 见决策 C-17。
- **`agent prompt` 的状态返回值不可信**（P14）：5/5 次返回投递前的 `idle`。
- **hermes 的 self-improvement 复现**（P3）：V8 交付后又改写了自己的私有 skill（还新增了一个 reference 文件）；
  本轮它没有影响项目文件，但仍是**协议外的持久副作用**。
- **`prepare --parent-depth` 语义坑**（P12）：`--parent-depth 1` 产出 `depth: 2`；
  我按 V8 的记录想产出 depth 1，试错一次后改 `--parent-depth 0` 才对齐（并删掉了误建的 round 目录）。
- **共享 checkout 纪律有效**：验证者遇到的 2 次瞬时编译失败（`src/cli/cat_file.rs`、`src/diff/myers.rs`
  正被并发作者编辑）都被正确标注为「不可归因于本任务」并等 30 秒重试，没有互相改文件。

## 5. 成本（原生计数，见 `.orch/waves/W2/cost-ledger.md`）

W2 起改用 skill 自带的 `scripts/usage.py` 走**原生导入**（`inspect-native` + `import-native`）：

| 轮次 | host | calls | input（含 cached） | cached | output |
|---|---|---|---|---|---|
| T6 作者 | omp | 90 | 16,138,242 | 16,034,048 | 169,972 |
| T7 作者 | codex | 42 | 3,051,924 | 3,006,080 | 64,736 |
| T6b 返工 | omp | 38 | 2,922,759 | 2,874,240 | 47,054 |
| V6 验证 | codex | 53 | 4,372,612 | 4,326,400 | 80,969 |
| V6b 复验 | codex | 40 | 2,431,254 | 2,385,536 | 43,810 |
| V7 验证 | omp | 33 | 3,178,137 | 3,119,744 | 51,926 |
| **合计** | — | **296** | **32,094,928** | **31,746,048（98.9%）** | **458,467** |

- **hermes / opencode 没有原生适配器**（`--host` 只支持 codex/omp），只能用面板的**上下文占用**读数，
  与上表**不是一个量纲**，因此本 wave 不做跨 kind 的成本对比（记入 SKILL-FINDINGS）。
- 采集本身很顺：6 个 manifest、`unmapped_calls=0`、重复导入幂等；
  代价是**必须逐轮手工建立 native session ↔ round 的映射**。

## 5b. W2 收口时的门禁（由 V6b 独立复跑，controller 引用）

`cargo test --offline` → **248 passed / 0 failed**（lib 146 + interop 7 + verify 3 +
verify_diff 16(+1 ignored) + verify_diff_paths 6 + verify_index 11 + verify_mergebase 10 +
verify_odb 6 + verify_pktline 8 + verify_refs 9 + verify_worktree 25 + doc 0）；
`cargo clippy --offline --all-targets` → 0 warning；`scripts/check-freeze.sh` → `21 file(s), drift 0`。
（W2 的 4 个任务 + 1 次返工全部落在白名单内：`src/odb/loose.rs`、`src/cli/cat_file.rs`、
`src/diff/{myers,unified}.rs`、`src/cli/diff.rs`、`src/merge/merge_base.rs`、
`src/transport/pktline.rs`。）

## 6. 结论与下一步

- **W2 证明了交叉验证不是走过场**：4 个任务里 1 个被验证者判 FAIL，且 FAIL 的理由是
  「作者矩阵没覆盖的一条真实 git 行为」，返工只花了 3m10s。
- **任务包质量是本轮最大的可控变量**：W2 出现的 3 个规格/口径问题（tree mode 位数、
  `--no-renames`、`ls-remote` 真值）全部出在 controller 写的任务书上，不在 agent 身上。
- 下一步（W3）：T9 pack / T10 three-way / T11 物化+分支命令 / T12 日常命令，
  任务包已写好（`.orch/waves/W3/`），**所有验证者都强制要求变异测试**（补 W2-S1 的缺口）。
