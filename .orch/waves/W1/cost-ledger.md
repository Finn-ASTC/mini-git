# W1 原生用量账本（native usage ledger）

> 规则：**只记各 target 自己上报的原生计数**，不做字节→token 估算，也不把缓存输入重复计两次。
> 采集时间：2026-09-19 20:0x +08:00（会话结束后从各工具的本地存储读出，非 TUI 快照）。

## 数据来源（可复现）

| kind | 来源 |
|---|---|
| omp | `~/.omp/agent/sessions/-Projects-mini-git/*.jsonl` 里每条 assistant message 的 `usage` |
| hermes | `~/.hermes/state.db` 的 `session_model_usage`（按 `task` 分桶：主任务 / `approval` / `background_review`）|
| opencode | `~/.local/share/opencode/opencode.db` 的 `message` 表（`$.tokens`）|
| codex | `~/.codex/sessions/2026/09/19/rollout-*.jsonl` 的 `total_token_usage`（会话累计）|

## 逐轮次

| 轮次 | kind | agent | 原生计数 | 成本 |
|---|---|---|---|---|
| T1 作者（试运行）| omp | `orch-omp-t1` | 44 条 assistant msg，累计 `totalTokens` **3,532,777**（in 49,306 / out 54,991 / cacheRead 3,428,480）| **$0.0507** |
| V1 验证者（试运行）| codex | `orch-codex-v1` | 会话累计 **1,522,634**（in 1,488,230 / out 34,404 / reasoning 20,620）| 未上报 |
| T2 作者 | hermes | `orch-hermes-t2` | 主任务 38 次 API：in 61,204 / out 59,226 / cacheRead 2,937,216 / reasoning 40,694；另有 `approval` 4 次、`background_review` 7 次、title 1 次（合计 ≈50 次调用）| 主任务 **$0.0535**；+审批 $0.0006 +后台复盘 $0.0104 = **≈$0.065** |
| V2 验证者 | omp | `orch-omp-v2` | 45 条 assistant msg，累计 `totalTokens` **3,663,068**（in 50,018 / out 49,018 / cacheRead 3,564,032）| **$0.0476** |
| T3 作者 | opencode | `orch-opencode-t3` | 34 条 msg：in 85,530 / out 19,314 / reasoning 56,554 / cacheRead 3,413,888 | **$0.07**（TUI 上报）|
| V3 验证者 | codex | `orch-codex-v3` | 会话累计 **4,036,319**（in 3,966,742 / out 69,577 / reasoning 48,552）| 未上报 |
| T4 作者 | codex | `orch-codex-t4` | 会话累计 **4,203,269**（in 4,113,949 / out 89,320 / reasoning 62,924）| 未上报 |
| V4 验证者 | hermes | `orch-hermes-v4` | 主任务 38 次 API：in 113,696 / out 62,267 / cacheRead 4,435,968 / reasoning 39,936；另有 `approval` 2 次、`background_review` 7 次 | 主任务 **$0.0677**；+审批 $0.0003 +后台复盘 $0.0129 = **≈$0.081** |

**本轮 6 个被编排 agent 的可见成本合计 ≈ $0.31**（codex 两轮只有 token 计数、无金额上报）。

## 观察

1. **账单大头是「缓存输入」而不是输出**：hermes 主任务 out 59K，但 cacheRead 2.94M；
   omp `totalTokens` 3.5M 里 97% 是 cacheRead。所以「按 token 总量比较 kind」会严重误导，
   必须把 `input / output / cacheRead / reasoning` 分开看。
2. **hermes 有协议外成本**：每轮结束后固定多花 **7 次 `background_review` 调用**
   （T2 $0.0104、V4 $0.0129），这正是 D3（自动改私有 skill）的账单。
   主任务的 38 次里还要再扣掉 `approval` 与 title 之外的部分。
3. **codex 单轮 token 量最大**（4.0–4.2M 会话累计，含 reasoning 49–63K），
   与其「审批往返多 + 每轮读取大量上下文」的行为一致；但它的本地存储**不记金额**。
4. **omp 的 `$` 与 hermes 的 `$` 口径不同**（omp=API 计价，hermes=`estimated_cost_usd`
   基于官方文档快照），跨 kind 比较金额只能当量级参考。
5. **controller（本会话）自身消耗 32.5M tokens**（codex rollout `18:49:00`，会话累计）——
   编排的成本不只是子 agent；监督、读屏、复跑门禁都在同一本账上。
