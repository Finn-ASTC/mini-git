# Wave W3 复盘（T9–T12：4 个 kind 并行 + 交叉验证 + 1 次返工 / 1 次复验）

- 时间：2026-09-19 20:47:05 → 21:43:00 +08:00（wall **55m55s**）
- 任务数：**4 个实现任务 + 4 个交叉验证任务 + 1 个返工轮（T11b）+ 1 个复验轮（V11b）**
- 参与 kind：**hermes / omp / codex / opencode**（4 个 kind 全部上场，作者 kind ≠ 验证 kind）
- 环境：herdr 用户既存 `default` 会话（**可见模式**），每 agent 一个可见 workspace（`--no-focus`），
  工作区策略 A（共享 checkout、目录互斥）
- 运行模型：**两段式 + 单点返工** —— 4 个作者同时开工 → 交付后各起独立 space 的验证者 →
  V11 判 FAIL → T11b 返工 → **在 V11 的同一 native 会话里续开 V11b 复验**

| 任务 | 写作用域 | 作者 | 验证者 |
|---|---|---|---|
| T9 pack 读取（idx v2 + 对象头 + OFS/REF delta） | `src/odb/pack/**` | **codex** | **omp**（V9） |
| T10 三方合并 + `mg merge` | `src/merge/three_way.rs`、`src/cli/merge.rs` | **opencode** | **hermes**（V10） |
| T11 物化 + branch/switch/checkout/reset | `src/worktree/materialize.rs`、`src/cli/{branch,switch,checkout,reset}.rs` | **omp** | **codex**（V11） |
| T12 add/rm/status/commit/log/tag | `src/cli/{add,rm,status,commit,log,tag}.rs`、`src/worktree/status.rs` | **hermes** | **opencode**（V12） |

## 1. 逐任务结果

| 轮次 | agent / pane | 提交 → 结果 | 用时 | 首位判定 | 备注 |
|---|---|---|---|---|---|
| T9 | `w3-t9-codex` / `w15:p1` | 20:47:30 → 21:00:07 | 12m37s | — | pack 三条读路径全对拍；主动列出「不做 crc32 校验」等已知限制 |
| T10 | `w3-t10-opencode` / `w16:p1` | 20:47:30 → 20:58:41 | 11m11s | — | 与 `git merge-file` 逐字节对拍（含 `--diff3`）；rename 检测列为非目标 |
| T11 | `w3-t11-omp` / `w17:p1` | 20:47:40 → 21:04:58 | 17m18s | — | 自建 95 项端到端对拍 + **300 例随机差分**，自查出 4 类偏差并修掉 |
| T12 | `w3-t12-hermes` / `w18:p1` | 20:47:40 → 21:05:30 | 17m50s | — | **报 blocked**（两个白名单外过期测试），交付完整；controller 用 C-18/C-19/C-20 接住，未开返工轮 |
| V9 | `w3-v9-omp` / `w19:p1` | 21:02:30 → 21:11:30 | 9m00s | **PASS** | 每个 pack 对象逐字节对拍；8 类反例全 `Corrupt`；4 个变异体全检出 |
| V10 | `w3-v10-hermes` / `w1A:p1` | 21:02:40 → 21:18:21 | 15m41s | **PASS** | 20 组语料 × 2 风格 + 400 组随机对拍；抓到 **zdiff3 退化**（W3-D6）与 5 处任务书真值误写 |
| V11 | `w3-v11-codex` / `w1B:p1` | 21:12:00 → 21:18:52 | 6m52s | **FAIL** | 抓到 **symlink 祖先删除越界（安全）** + 写路径不一致（W3-D1/D2），留 `#[ignore]` 回归用例 |
| V12 | `w3-v12-opencode` / `w1C:p1` | 21:12:10 → 21:15:59 | 3m49s | **PASS** | 95 项独立对拍；明确「不存在 T12 交付缺陷」并逐条区分未覆盖 |
| **T11b（返工）** | `w3-t11b-omp` / `w1D:p1` | 21:23:00 → 21:29:57 | 6m57s | — | 只改 `src/worktree/materialize.rs`；**质疑任务书前提**并做 42 格夹具矩阵，改后 42/42 与 git 一致 |
| **V11b（复验）** | `w3-v11b-codex` / `w1B:p1`（**复用 V11 会话**） | 21:34:25 → 21:43:00 | 8m35s | **PASS** | 23/23 格自跑矩阵；两个变异体（删守卫 / 拒换真目录）均检出；421 passed / 0 failed |

首轮通过率：**3/4 = 75%**（V9/V10/V12 PASS；V11 FAIL，走返工 + 复验）。

## 2. 核心指标（§6）

| 指标 | 数值 | 说明/证据 |
|---|---|---|
| 协议有效率 | **12/12 = 100%** | 12 个 round 的 `protocol.py validate --check-files` 全 exit 0；`job_id`/`round_id`/result 路径零错配 |
| **假绿率** | **0** | 4 个作者声称的测试数量与结论，独立复跑全部成立；V9 额外发现作者的用例在 git 缺失时会**静默通过**（已点名，见 W3-D7） |
| 越界率 | **0 个文件** | mtime 窗口（20:47→21:21）里的 `src/**` 改动与四位作者声明的白名单**逐文件吻合**；V11b 只动 `tests/verify_materialize.rs`；`Cargo.toml`/`Cargo.lock`/`mod.rs` 未变 |
| 接口漂移次数 | **0** | `scripts/check-freeze.sh` → `checked 21 file(s), drift 0`（V11b 与 controller 各复跑一次）。FREEZE 由 v0.4 → **v0.5**（C-18 改了 controller-owned 的 smoke 用例） |
| blocked 处理 | 协议级 blocked **1**（T12） | 问题具体（给出两个一行级修法）+ **没有越界改**；controller 用 C-18/C-19 修完复跑全绿，按「blocked 但工作完成」记账（C-20），未开返工轮 |
| 交叉验证真实率 | **5/5 真实执行** | V9/V10/V11/V12/V11b 都新建或重写了独立 test target，真值零硬编码，全部含变异测试 |
| 并行加速比 | **作者段 3.20×**；验证段 2.16× | Σ作者 3536s / 作者段 wall 1105s（20:47:05→21:05:30）；Σ验证 2122s / 验证段 wall 982s（21:02:30→21:18:52） |
| 返工轮次占比 | **2/12 = 16.7%** | 1 个返工（T11b）+ 1 个复验（V11b），合计 15m32s（占全 wave 28%）|
| token 用量（原生计数） | 见 §5 | codex/omp 有适配器；hermes/opencode 无（P15） |

## 3. 验证者的独立发现（本轮最大价值）

1. **W3-D1（安全，真缺陷）**：物化删除路径跟随 symlink 祖先，删掉工作区外的数据。
   V11 给出可一行行复跑的最小复现，并在自己的测试里用 `#[ignore]` 钉住，避免把共享 checkout 弄红。
2. **W3-D2（一致性，真缺陷但有夹具变量）**：写路径遇 symlink 祖先时，mg 是否应与 git 一样放行，
   **取决于「顺 symlink 能否 stat 到目标」**——这个变量 V11 没写、controller 复跑时换了值，
   于是双方各自「正确」地得出相反结论（见 §4 与 `SKILL-FINDINGS.md` P18）。
3. **W3-D6**：V10 发现 `merge.conflictstyle=zdiff3` 退化成 diff3（作者自述「支持 zdiff3」不成立）
   → 判定为已确认的已知限制（C-22）。
4. **W3-D7**：V9 发现作者用例的「环境缺失即 return」写法（假绿风险）。
5. **V12** 把「T12 交付缺陷」与「controller 侧过期测试」严格分开，为 C-20 的记账方式提供了依据。

## 4. 本 wave 最重要的方法论产出：真值必须带夹具

- V11 报 FAIL 2 → controller 换夹具复跑 → 判「V11 测错了」并写进返工任务书（**v1 更正**）→
  作者 T11b 拒绝照做，做 42 格矩阵 → controller 再复跑 → **发现 v1 更正自己也错了**。
- 正确形态：`夹具状态 → 行为` 的**矩阵**，而不是一句「真实 git 会 X」。
- 已固化：`SKILL-FINDINGS.md` **P18**；W4 三份验证书都加了「夹具必须写全」条款；
  三份任务书都加了「前提可被质疑，先报冲突再动手」。

## 5. token 用量（原生计数，可复核）

| 轮次 | kind | calls | input | cached | output | 来源 |
|---|---|---|---|---|---|---|
| T9 | codex | 55 | 6,323,144 | 6,274,432 | 117,179 | rollout `sess_0016…` |
| T11 | omp | 175 | 35,422,506 | 35,271,040 | 199,624 | omp session `sess_0017…` |
| V9 | omp | 59 | 7,519,306 | 7,440,256 | 100,617 | omp session `sess_0018…` |
| V11 | codex | 63 | 7,149,355 | 7,077,888 | 114,009 | rollout `sess_0019…` |
| T11b | omp | 61 | 7,874,925 | 7,800,192 | 107,605 | omp session `sess_0020…` |
| V11b | codex | 77 | 7,826,663 | 7,597,312 | 97,274 | 同一 rollout `sess_0019…`（第 2 turn） |
| **合计（6 轮）** | | **490** | **72,115,899** | **71,461,120（99.1%）** | **736,308** | manifest 存 `.orch/artifacts/usage-manifests/*.json` |

- **未测**：T10/V12（opencode）、T12/V10（hermes）—— 方案没有这两家的原生适配器（P15），
  面板读数只是上下文占用，不能当累计用量。
- **新发现**：`usage.py` 的 `PURPOSES` 只有 `task/retry/report_repair`，
  没有 `verify`/`rework` 词表（P19）；V11b 与原 V11 在同一个 native session 里是**两个 turn**，
  能按 `native_id` 精确切分（复验轮成本可单独计量）。
- 对照 W2（codex/omp 6 轮）：296 calls / input 32,094,928 / cached 31,746,048 / output 458,467。
  W3 的 input 是 W2 的 2.25 倍，主要来自 T11 一轮的 175 calls（35.4M input）。

## 6. 注入场景与观察

| 场景 | 是否触发 | 实际行为 | 与期望的差异 |
|---|---|---|---|
| S1 真缺陷注入 | 自然发生（W3-D1） | 验证者用平行仓库对拍抓到安全级缺陷，并留 `#[ignore]` 用例 | 无 |
| S9 越界诱导 | 未主动注入，自然出现 | T12 撞上白名单外过期测试 → **报 blocked、不越界** | 无（这正是期望行为） |
| 前提被推翻 | 自然发生（W3-D2） | 作者拒绝照做，报冲突并给 42 格矩阵 | 方案未定义「作者质疑任务书」的正式入口（已用 result 文本承载） |
| 续轮复用会话 | 主动使用（V11→V11b） | 同一 codex 会话、新 round/result，身份不串；审批仅 1 次 | 无（建议写进 skill 的复验流程） |

## 7. 结论与模板改动

- **产品**：W3 修掉 1 个安全级缺陷 + 3 类一致性缺陷；mini-git 现有 421 个测试（1 个 `#[ignore]`），
  clippy 0 warning，FREEZE drift 0。
- **skill 缺陷（本 wave 新增）**：P18（真值缺夹具）、P19（`PURPOSES` 无 verify/rework 词表）；
  P17 得到 3 个实例并被 W4 模板固化；P3/P4/P5/P13 各再现一次。
- **下一 wave 的模板改动**（已写入 W4 文件）：
  1. 任务书/验证书都必须写「前提可被质疑，先报冲突再动手」；
  2. 「真实 git 会 X」必须写成夹具矩阵；
  3. 禁止「环境缺失即 return 通过」，必须硬失败或显式 `#[ignore]`；
  4. 复验轮优先复用原验证者的 native 会话（成本更低、上下文连续）。
