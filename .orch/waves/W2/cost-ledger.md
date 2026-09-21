# Wave W2 成本账本（原生计数）

> 采集方式：`agent-orchestrator` skill 自带的 `scripts/usage.py`。
> **codex / omp 用的是 native JSONL 导入路径**（`inspect-native` + `import-native --manifest`），
> 每个 native 调用一条 delta 样本，落在各轮 `usage/calls/` 下，可用 `usage.py summary --request <request.json>` 复核。
> **hermes / opencode 没有原生适配器**（skill 目前只支持 `--host codex|omp`），只能用 TUI 面板读数。
> 本文件不含任何按字节/行数估算的 token。
>
> 采集时间：2026-09-19 20:55 +08:00（W3 已在并行进行，本账本只覆盖 W2 的六个 codex/omp 轮次）。

## 1. codex / omp：原生计数（可复核）

| 轮次 | round 目录 | native calls | input_tokens（含 cached） | cached_input_tokens | output_tokens |
|---|---|---|---|---|---|
| T6 作者 | agent-orchestrator-tezp4suy | 90 | 16,138,242 | 16,034,048 | 169,972 |
| T7 作者 | agent-orchestrator-ce6l9t1x | 42 | 3,051,924 | 3,006,080 | 64,736 |
| T6b 返工 | agent-orchestrator-yt9iu4s5 | 38 | 2,922,759 | 2,874,240 | 47,054 |
| V6 验证 | agent-orchestrator-6tv40dmx | 53 | 4,372,612 | 4,326,400 | 80,969 |
| V6b 复验 | agent-orchestrator-ngtbxt6j | 40 | 2,431,254 | 2,385,536 | 43,810 |
| V7 验证 | agent-orchestrator-4xmt84od | 33 | 3,178,137 | 3,119,744 | 51,926 |
| **合计** | 6 轮 | 6 | 32,094,928 | 31,746,048 | 458,467 |

复核命令（任选一轮）：

```bash
python3 ~/.agents/skills/agent-orchestrator/scripts/usage.py summary \
  --request .orch/rounds/W2/agent-orchestrator-tezp4suy/request.json
```

导入用的 manifest 与原生日志路径（可重放，重复导入是幂等的）：

| 轮次 | host | native session |
|---|---|---|
| T6 作者 | omp | `~/.omp/agent/sessions/-Projects-mini-git/2026-09-19T12-13-15-961Z_sess_0010-c739-7735-8628-1a95183de73b.jsonl` |
| V7 验证 | omp | `~/.omp/agent/sessions/-Projects-mini-git/2026-09-19T12-30-55-579Z_sess_0013-f25b-7574-9fcd-d2cdc114f579.jsonl` |
| T6b 返工 | omp | `~/.omp/agent/sessions/-Projects-mini-git/2026-09-19T12-39-49-436Z_sess_0014-17bc-76ef-92e1-b2b9c9974361.jsonl` |
| T7 作者 | codex | `~/.codex/sessions/2026/09/19/rollout-2026-09-19T20-13-18-sess_0010-d2c1-72e0-8c38-2dda7071377e.jsonl` |
| V6 验证 | codex | `~/.codex/sessions/2026/09/19/rollout-2026-09-19T20-30-13-sess_0013-4cb8-77c3-ab4c-578be7fff523.jsonl` |
| V6b 复验 | codex | `~/.codex/sessions/2026/09/19/rollout-2026-09-19T20-45-21-sess_0015-285d-75c3-b55c-9275b4c6c9f8.jsonl` |

**读法**：`input_tokens` 已包含 `cached_input_tokens`（不要重复相加）。cached 占比 98.9%，
这是「长上下文 agent 反复重读同一批文件」的典型形态，不是幻觉数据。

## 2. hermes / opencode：只有面板读数（skill 无原生适配器）

| 轮次 | kind | 面板读数 | 说明 |
|---|---|---|---|
| T5 | hermes | ~71K / 1M 窗口 | 面板显示的是**上下文占用**，不是累计调用量；不可与上表相加 |
| V8 | hermes | ~71.3K / 1M | 同上；另有 7 次固定 `background_review`（self-improvement）调用 |
| T8 | opencode | 139.8K（14%）/ $0.04 | 同上是上下文占用口径 |
| V5 | opencode | 未记录 | opencode 完成后未截取面板 |

## 3. 结论（对测试床的意义）

1. **原生计数可用且可复核**（codex/omp 各 6 轮全部导入成功，`unmapped_calls=0`），
   但**只能在轮次结束后**用 manifest 显式映射，controller 需要先确认 native session 与 round 的对应关系。
2. **hermes / opencode 的 token 无法用同一口径采集** → 跨 kind 的成本对比目前**不成立**，
   只能给出「按 kind 的量级感」。这是 skill 的一条真实缺口（已记入 `.orch/SKILL-FINDINGS.md`）。
3. 面板读数与原生计数**不是一个量纲**（前者是上下文占用，后者是累计调用量），
   W1 的账本混用了两者，W3 起应统一用原生导入。
