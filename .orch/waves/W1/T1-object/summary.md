# Wave W1 / T1 复盘（试运行：omp 写 → codex 验）

> 状态：**试运行完成**。T1 由 omp 实现，V1 由 codex 独立验证并判定 PASS，
> controller 侧也独立复跑通过。结论与改进项见 §4。
> 本文件是 §6 指标的第一批真实数据，模板见 `.orch/templates/wave-summary.md`。

## 1. 轮次记录

| 项 | T1（作者） | V1（验证者） |
|---|---|---|
| kind | omp | codex |
| agent / pane | `orch-omp-t1` / `w1:p1` | `orch-codex-v1` / `w2:p1` |
| job_id | `edb0e351b58e42e486a264690ece2dcd` | `816ca880b03a437c8aa3da7cec8b3cf9` |
| round_id | `c1008bbd20e34640ab5a409ceed097fb` | `c1431df7a41e4b2fa6e43dd022e2acb5` |
| depth | 1（max_depth 3） | 1 |
| transport | isolated herdr 会话 `orch-w1-t1` | 同会话，第二个 workspace，`owns_session=false` |
| 提交 | 19:13:40 +08:00 | 19:18:45 +08:00 |
| 结果 | `success`，19:18:04 +08:00（约 4.5 分钟） | `success`，19:29 左右（约 10 分钟） |
| result 路径 | `../../../rounds/W1/agent-orchestrator-1vhlii69/result.json` | `../../../rounds/W1/agent-orchestrator-clc6i26t/result.json` |

任务包与证据：`task.md`（T1）、`verify-task.md`（V1）、`baseline.txt`（开工前快照）、
`controller-goldens.txt`（controller 自己用真实 git 生成的真值）、`decision.md`（原生 UI 决策）、
`omp-result.json` / `request.json`（不可变协议证据副本）。

## 2. 核心指标（§6）

| 指标 | 数值 | 证据 |
|---|---|---|
| 协议有效率 | 1/1 = 100% | `validate --check-files` 退出码 0；identity/schema/路径全部匹配 |
| **假绿率** | 0 | 作者声称 29 个 lib 测试通过 → controller 复跑一致；验证者也独立复跑一致 |
| **越界率** | 0 个文件 | baseline 哈希对拍：改动仅 `src/object/{tree,commit,tag}.rs` 三个白名单文件 |
| 接口漂移次数 | 0 | `scripts/check-freeze.sh` → `checked 21 file(s), drift 0` |
| 交叉验证真实率 | 真实执行 | 验证者重新生成 7 条 golden 真值逐条比对 + 新增运行时 oracle 测试 `tests/verify/main.rs`（3 例）|
| 验证者独立性 | 成立 | 新增测试**零硬编码真值**（运行时调真实 git 对拍）；且未改 `src/**` |
| 验证者诚实度 | 好 | 主动标注唯一无法 100% 字节复现的 `MERGE_PAYLOAD`（parent oid 依赖原作者仓库历史）并说明为何不构成可疑；还主动披露自己测试首版写错过一处断言 |
| 真值一致性（controller 侧） | 一致 | agent 报的 tree oid `4aad58e0401292349b1a142aa10cbcc061c2eda2` == controller 独立生成的同一值 |
| 并行加速比 | 不适用（单任务试运行） | — |
| 返工轮次占比 | 0（尚无 follow-up） | — |
| token 用量 | omp：状态行显示 104K / 上下文 11% / $0.05（快照，非 `/usage` 精确值） | pane 状态行；未用字节估算 |

## 3. 过程观察（编排 skill 的真实行为）

1. **herdr 状态不可信（重要）**：omp 启动后 `agent get` 一直返回 `screen_detection_skipped: true`，
   提交任务后仍显示 `idle`、`revision` 不变，但 pane 里 spinner 与状态行证明它已在工作。
   若 controller 只看 `agent get`，会误判「提交没成功」并可能重复提交（协议禁止）。
   → 本轮的判定依据是 `pane read` + 状态行 + 结果文件，而不是 herdr 的 agent 状态。
2. **omp 会在轮次结束后弹产品级对话框**：result 已发布后才出现「要不要做 10-15 分钟学习练习」Ask 弹窗，
   并把状态置为 `blocked`（标题 `π !`）。这与「协议级 blocked」是两回事：
   后者需要新轮次，前者只是原生 UI，需 controller 现场处理（见 `decision.md` D1）。
3. **codex 对复合 bash 命令逐条弹原生审批**：本次验证出现 3 次
   「1 Yes, proceed / 2 don't ask again for this prefix / 3 No」。
   全部按「读实际命令 → 确认在授权范围内 → 选 1」处理，**未使用选项 2**（不做常驻批量授权）。
4. **观察者噪声偏高**：omp 的 TUI 持续重绘（spinner、进度行），`watch.py` 的 changed-screen
   启发式共产生 8 次 `review_due`，其中 7 次是纯噪声（逐个读屏确认无弹窗）。
   对持续重绘的 TUI，`review_due` 的信噪比需要调参或降频。
5. **错误示范（controller 侧，需记录）**：我用 `pkill -f "watch.py run --watch /tmp/orch-watch-x90zk5ag"`
   停旧观察者时，模式匹配到了自己的命令行，把自己的 shell 一起杀了（退出码 143）。
   教训：`pkill -f` 的模式必须自我排除（如 `[w]atch.py`），或改用受管进程句柄。
6. **跨 kind 差异（本轮最有价值的观察）**：
   | 维度 | omp（作者） | codex（验证者） |
   |---|---|---|
   | herdr 状态可信度 | 差：`screen_detection_skipped=true`，工作期间仍报 `idle` | 好：`working`/`blocked`/`done` 与实际一致 |
   | 原生对话框 | 轮次**结束后**弹产品级「学习练习」Ask，把状态置为 `blocked` | 执行期间弹命令审批（每次一条复合命令） |
   | 结束状态 | `idle`（标题 `π >`） | `done` |
   | 收敛速度 | 4.5 分钟 | 10 分钟（含 3 次审批往返） |

## 4. 结论与待办

- T1 的产物在 controller 独立复核下成立（真值一致、无越界、无假绿）。
- V1 结论：**PASS**。验证者独立重生全部 golden 真值（7 条全部一致）、确认断言真的在跑被测代码
  （没有 `contains`/`#[ignore]`/自造期望值）、并用运行时真 git 新增了 3 个 oracle 测试
  （排序陷阱 `foo.txt` vs `foo/`、tree/commit/tag 字节往返、merge 双 parent）。未发现缺陷 → 假绿率 0。
- 遗留（非阻塞）：`MERGE_PAYLOAD` 的父提交 oid 无法脱离作者当时的历史独立复现，
  已由运行时 merge 用例覆盖；后续任务包可要求作者同时给出可复现的生成脚本。
- 试运行结论（用于决定是否放大到 W1 全量 4 路并行）：
  **协议链路可用，但监督成本集中在「原生 UI 对话框」与「持续重绘 TUI 的噪声」两点**，
  放大前建议：① 任务包显式写明「不要使用学习练习/额外交互」；
  ② 观察者对持续重绘目标降频或只对关键词事件告警。

## 5. 成本（试运行实测）

| 项 | omp（作者） | codex（验证者） |
|---|---|---|
| 端到端耗时 | 约 4.5 分钟 | 约 10 分钟 |
| controller 介入次数 | 1 次（学习练习弹窗） | 3 次（命令审批） |
| controller 工具调用（含监督） | 约 12 次 | 约 8 次 |
| 原生 token 快照 | 上下文 104K / 11% / $0.05 | 未采集（TUI 状态行未显示用量） |
