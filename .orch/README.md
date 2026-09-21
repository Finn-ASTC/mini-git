# `.orch/` —— 编排产物目录（证据，纳入版本管理）

本目录是**异构 agent 编排测试床**的运行记录。它回答的问题不是「代码写完了吗」，
而是「四个不同 agent 在受控协议下协作时，行为是否可观测、可验证、可复现」。

## 目录布局

```
.orch/
├── FREEZE-v0.md           # W0 接口冻结清单（哈希基线，代替 git tag；现为 v0.6）
├── README.md              # 本文件
├── SKILL-FINDINGS.md      # ★ 测试床的主要产出：P1–P24 编排 skill 缺陷清单
│                          #   （§0.1 是 2026-09-20 的上游现状核查总表：哪条已修、哪条还是待办）
├── templates/
│   ├── task.md            # 任务包模板（§5 要求的六项）
│   └── wave-summary.md    # wave 复盘模板（含指标表）
├── artifacts/             # ★ /tmp 的抢救性归档（工具/审批日志/token 清单/harness/变异体）
│   ├── README.md          # 归档索引：每一项的来源、用途、关联的 P/D 编号
│   ├── PROVENANCE.md      # 每个文件的原始 /tmp 路径 + mtime + sha256
│   ├── tools/  logs/  usage-manifests/  preflight/  harnesses/  mutants/  notes/
│   ├── release-p24/       # release 专项测试现场（P23/P24 的原始证据 + 最小复现）
├── rounds/W<n>/<round_id>/    # ★ 每一轮的协议原文（比 waves/ 更权威）
│   ├── request.json       # protocol.py prepare 生成：job_id/round_id/result_path/cwd/depth
│   ├── prompt.txt         # 实际送进 agent 会话的完整 prompt
│   ├── resources.json     # 该轮占用的 herdr pane/workspace/tab
│   ├── result.json        # 目标 agent 写：status/output/files_*/error/blocked_reason
│   └── usage/             # 原生 token 计数（codex/omp；hermes/opencode 无适配器）
└── waves/
    ├── W<n>/<Tn>-<name>/
    │   ├── task.md        # 任务包（objective/scope/acceptance/非目标）
    │   ├── verify-task.md # 验证任务书（kind ≠ 作者）
    │   ├── baseline.txt   # 开工前的 git status --porcelain + HEAD + 文件清单
    │   ├── verify-scratch/# 验证者的临时夹具与原始输出
    │   └── decision.md    # 发生 blocked 时的问答原文
    ├── W<n>/summary.md    # 每 wave 复盘（指标表 + token 账本 + 验证者发现）
    └── W<n>/defects.md    # 产品缺陷（W<n>-D*）+ 流程缺陷（W<n>-S*）
```

> **W1–W4 全部收口**（2026-09-19）：`summary.md` / `defects.md` 四个 wave 均已写完；
> 全仓门禁 **562 passed / 0 failed / 1 ignored**、`clippy` 0 warning、`check-freeze.sh` drift 0。
>
> **W5**（2026-09-20）：用更新后的 skill 另起一次性项目 `/tmp/w5-linestat` 跑真实异构两轮（omp 作者 + opencode 验证者）。
> **W6**（2026-09-21）：上游 C1/C2/B2 批的**脚本级**实测 —— 不起子 agent、不花 token，直接对 W5 留下的真实 run 取证；
> 产物见 `waves/W6/summary.md` 与 `waves/W6/evidence/`。

## 协议要点（与 agent-orchestrator skill 一致）

* 工具：`ORCH_TOOL=$HOME/.agents/skills/agent-orchestrator/scripts/protocol.py`（Python 3.10+）。
* 准备一轮：

  ```bash
  python3 "$ORCH_TOOL" prepare --cwd "$PROJECT_CWD" \
    --parent-depth <我收到的 depth> --task-file .orch/waves/W1/T1-xxx/task.md --brief
  ```

* 校验一轮（exit 0 = 协议有效，含 `blocked`/`error`；exit 2 = 文件缺失；exit 3 = 数据非法）：

  ```bash
  python3 "$ORCH_TOOL" validate --request <request.json> --check-files
  ```

* **一轮 = 一个 `round_id` + 一个 result 路径**。`blocked` 的答复是**新一轮**，不能复用旧路径。
* prompt 里必须带**绝对 result 路径与完整上报契约**；环境变量只是提示，不能替代 prompt。
* 同一目标同时只能有一个在途轮次；`idle` 只表示「可接受输入」，不等于任务成功。
* 写作用域互斥：一个 agent 一个目录；`FREEZE-v0.md` 里的文件不可改（越界 = S5 场景，要记录）。

## 每轮必须留的证据

1. `baseline.txt`：开工前的工作区状态（防止把用户既有改动算到 agent 头上）。
2. `result.json`：协议化结果（不合格的 result 本身就是「协议有效率」的数据点）。
3. 验证者的**独立复跑**证据：原始命令与输出（在本项目的实现里落在 `.orch/rounds/<round>/result.json` 与
   `.orch/waves/W<n>/<Tn>/verify-scratch/**`）—— 只复述作者结论不算验证；
   推荐同时冻结被验证制品的 sha256（W4/V16 的做法，见 `SKILL-FINDINGS.md` P21）。
4. `decision.md`：`blocked` 的原始问题与 controller 的答复（同轮答复属于违规，要记录）。

## 指标口径

见 `ORCHESTRATION.md` §6。每个 wave 结束时用 `templates/wave-summary.md` 汇总，
其中「假绿率」「越界率」「协议有效率」「接口漂移次数」是四个核心可信度指标，
`scripts/check-freeze.sh` 给出接口漂移的客观数字。
