# Wave W5 —— 用更新后的 skill 复验一轮真实异构编排

日期：2026-09-20。目标项目：`/tmp/w5-linestat`（全新，非 mini-git）。参与者：T1 作者（**omp 18.2.6**）+ V1 验证者（**opencode 1.18.29 / OMO**），控制器 codex。
**W5 不是 mini-git 的开发轮**，是 `agent-orchestrator` skill 更新后的一次端到端「还能不能跑、比以前少踩什么坑」的复验。
缺陷与观察见 [defects.md](defects.md)；跨 wave 合并视图见 `.orch/SKILL-FINDINGS.md`；
**给上游的 issue 清单**见 `.orch/upstream/2026-09-20-w5-upstream-issues.md`。

## 1. 这一轮到底在测什么

W1–W4 跑的是**旧版 skill**，暴露了 P1–P24。上游据此更新了 skill（新增 `references/delivery-configurations.md`、
`assets/verification/rust-cli-plan.json`、`scripts/supervision.py`、持久 run + `jobs.py` 提交索引、`runs/jobs/delivery/completion/reviews` 等）。
W5 用**新 skill** 跑一个真实小项目，逐条验证「新流程是否真的可用」，而不是看文档写了什么。

选 `linestat`（无第三方依赖的 Rust CLI，字节级 lines/words 定义，故意与 `wc` 不同）是为了**正压新版「交付配置」要求**：
必须同时声明并实跑 dev / release / release-artifact 三类配置，不能只报一个测试总数。

## 2. 时间线与实际动作

| 时刻（本地） | 事件 |
|---|---|
| 17:41:13 | 持久 run `orch-run-2lpx81iy` 建立；基线 `.orch/waves/W5/baseline.txt`（SPEC.md sha256 `20903f07…98244`） |
| 17:41:19 / 17:42:42 / 17:43:09 | T1 `prepare` → `jobs.py register/claim/update/begin` → receipt（`accepted / target_active`） |
| 17:43–17:45 | T1（omp）在 `/tmp/w5-linestat` 实现并跑完 dev+release 门禁 |
| 17:45:00.997 / 17:45:11.733 | T1 发布 `result.json`，**随后又原子重发了一次**（见 W5-S1） |
| 17:45:37 | **交付快照封存** `snapshot-nvn21yva`（6 文件，`result_sha256 643efc20…`） |
| 17:45:56 / 17:46:48 / 17:47:20 | V1 `prepare` → `begin` → prompt 单次发送 |
| 17:47:43 | **控制方独立门禁** `delivery.py verify --snapshot … --plan controller-plan.json` → `passed: true`，7/7 exit 0 |
| 17:49:45 | V1 发布 result（`status=success`，sha256 `3b35b7fd…50e935`） |
| 17:51:19–17:51:30 | 控制方封存 V1 证据快照 + 对 V1 证据跑控制方门禁（36/36） |
| 17:51:40 / 17:52:14 | T1 `accept`(delivery) → `close(completed)` |
| 17:51:41 / 17:52:15 | V1 `accept`(delivery) → `close(completed)` |
| 17:52:2x | 两个观察器停止；两个子 agent `/exit`；`w1S`/`w1T` 拆除，父 pane `wJ:p1` 保留 |

**端到端 11 分钟**（17:41:13 → 17:52:15），2 个异构 agent 轮，零返工轮。

## 3. 交付结果

- 产物：`target/release/linestat`，sha256 `759534809958403271ec37bba8962439ab51938999bdd214214b6bff3ea44f5e`。
- **三方独立得到同一哈希**：作者构建的制品、控制方从封存源码独立重建的制品、V1 自己构建的制品。
- 门禁：dev 测试 11/11、release 测试 11/11、`cargo fmt` 干净、`clippy -D warnings` 干净，
  release 制品行为检查由**验证者自写的 oracle**驱动 **36/36 通过**（不是作者的自测）。
- 控制方用**自己独立构建的制品**重跑验证者的 harness → **36/36**（`/tmp/w5-ctrl-xcheck/report.json`）。
- 产品结论：PASS。7 项明确的未覆盖项 + 4 项 SPEC 未定义行为被分列，未被打成 pass。

## 4. 成本

| 角色 | host | 输入 | 其中缓存 | 输出 | 原生来源 |
|---|---|---|---|---|---|
| T1 作者 | omp 18.2.6 | 1,487,387 | 1,451,008 (97.6%) | 32,135 | `~/.omp/agent/sessions/…jsonl`（27 calls） |
| V1 验证者 | opencode 1.18.29 | 1,510,971 | 1,440,896 (95.4%) | **34,572** | `~/.local/share/opencode/opencode.db`；另有 $0.0356 |
| **合计** | | **2,998,358** | 2,891,904 (96.4%) | **66,707** | |

记账限制与后续更正：W5 期间 `usage.py` 的 `purpose` 词表只有 `task/retry/report_repair`，**且没有 OpenCode 适配器**，
所以 V1 的验证轮只能记成 `task`、数字也要我手工换算。**其中 output 一项当时算错了**：把原生 `tokens_output`（16,776）
直接当成 output，漏加了 reasoning 细分（17,796）。

上游随后（kumi `a58905d`）补上了四种宿主适配与正交的 `role`/`phase` 维度。已用新的 `usage.py correct` 追加带证据的原子更正：
**V1 output 16,776 → 34,572**，并补 `role=verifier / phase=verification`；原样本文件不动，effective 版本生效。
上表为更正后的数字。证据见 `.orch/waves/W5/evidence/v1-opencode-native-usage.json`，逐条核对见 `.orch/SKILL-FINDINGS.md` §0.4。

## 5. 新版 skill 在本轮**首次真正生效**的能力

| 能力 | 实测结果 |
|---|---|
| 字母型 herdr 资源 ID（P1） | T1 拿到 `w1S`、V1 拿到 `w1T`；`protocol.py` 校验通过。旧版会直接拒收 —— **P1 修复在真实运行中生效** |
| 持久 run + 提交索引 | 全程 `orch-run-2lpx81iy`；18 次 `jobs.py` mutation 全部带 `--token` + `--expect-revision`，revision 单调递增，无冲突 |
| 交付快照 + 控制方独立门禁 | 两次封存（T1 交付、V1 证据）；控制方两次都在**独立构建目录**重建/重跑，而不是读对方的结论 |
| `result_changed` 监督 | **首次在真实产生时捕获**（W5-S1），W1–W4 从未出现过这类事件 |
| 交付配置覆盖（P23 / A 批） | dev / release / release-artifact 三类配置逐条给结论；「构建成功 ≠ 行为检查」被明确区分 |
| 不可变巡视回执 | T1 watch 10 事件、V1 watch 6 事件，全部按 `--expected-revision` 写回执；`pending` 收敛到 0 |
| `accept` 需要绑同一 job/round/request/result 的交付 attempt | 两个 job 都被工具要求提供 `attempt_path`，无法只凭一句「我看过了」通过 |

## 6. 与 W1–W4 相比少踩的坑

1. **可见子 agent 的建、查、清一条龙不再需要变通**：旧版靠 `record` 失败后的手工补偿；本轮 `record`/`cleanup-plan` 直接可用，清理命令是工具生成的（`protocol.py cleanup-plan`）。
2. **「作者说通过」和「验收通过」被工具强制分开**：W1–W4 靠 controller 自觉；本轮 `check-completion` 在缺 `acceptance` 和 `host` 时会明确返回 `ready_to_complete: false` 并给出原因。
3. **返工轮不再有「同一 job 无法续轮」的摩擦**（P2 已修，本轮未触发但路径存在）。
4. **监督噪声显著下降**（P6 已修）：V1 一轮 2.5 分钟只产生 6 个事件，其中 2 个是低置信 UI 提醒；旧版同类 TUI 会刷出大量 `review_due`。
5. **无原生审批**：本轮两个宿主都没弹审批（omp/opencode 都是 insider 直连），所以 P5「codex 逐条审批」的瓶颈本轮不可见 —— **不能据此说 P5 解决了**。

## 7. 本轮仍未覆盖 / 明确不成立的结论

- **单宿主样本**：只有 omp + opencode，hermes 与 codex 没上场；`openai-docs` 无关。
- **无审批路径**：没有出现原生权限弹窗，所以「等待一方时推进兄弟任务」这条在 W5 未受测。
- **无返工轮**：因为没发现产品缺陷，rework 模板与 `prepare --previous` 未被实跑。
- **没有并发第三个任务**：兄弟任务是顺序的（T1 发布后才起 V1），W4 那种「并行中间态」问题（P13/P21）本轮不可见。
- **记账归因仍是手工**（P15/P16/P19 原样）：本轮两个 host 的数据都拿到了，但把 native 会话映射到角色轮次仍靠人工。
- **`delivery.py capture` 的 baseline 未被使用**：T1 的交付快照 `baseline: null`，因此 `actual_diff` / `declaration_comparison` 也是 null；
  本轮靠 `result_sha256` 和逐文件哈希兜住，但「改了什么」这一列是空的 —— 记为方法缺口。

## 8. 结论

更新后的 skill 在真实异构 agent 上**可用**，且把 W1–W4 里靠人肉纪律维持的几件事（角色轮次身份、验收证据绑定、资源清理、变更检测）变成了工具行为。
本轮暴露的问题不再是「工具不能用」，而是**契约只禁止违规、不给补救出口**（W5-S1：作者明知 "never revise a valid response" 仍重发了已发布结果，因为它被发现错误后无路可走）与**交付捕获的静默降级**（W5-S3：不带 baseline 时 diff 列为 null 且无告警）。

> **更正记录**：W5-S1 初判为「规则只写在控制器一侧」，复核作者 `prompt.txt` 后确认该说法**不成立** —— `protocol.py:190` 已把该规则逐字下发。已按「已发布记录不追改」保留原判并另立此更正，P25 严重度由 🟠 降为 🟡。

## 9. 收口状态（终态核对）

| 项 | 结果 |
|---|---|
| `runs.py recover --run …/run.json` | `complete: true`、`errors: []`、`action_required: 0`、`waiting_user: 0` |
| 两个 job | `submission=accepted`、`closed.outcome=completed`（T1 rev 9 / V1 rev 9） |
| 巡视回执 | T1 watch 11 个事件、V1 watch 7 个事件，**全部**有不可变回执，无遗留 open |
| 更正 | W5-S1 / P25 的初判经复核后下调（见 §8 更正记录），原始判断保留在 `notes/` 与 `defects.md` 的历史行内 |
| 上游跟进 | 本轮的 W5-S3 / W5-S1 / W5-S2 已由上游 `b124b87` 修复并同步到运行时（vault `cc1a069`）；W5-S4（P19）归入上游 D 阶段。逐条核对见 `.orch/SKILL-FINDINGS.md` §0.3、`defects.md` 末节 |
| 补做 | 按上游新要求补跑了「验证者必须同时测故意错误输入」的负对照：故意答错的替身 36/36 全被拒、真制品 36/36 通过。见 `evidence/v1-acceptance-negative-control.md` |
| 观察器进程 | 两个都已停止（`observer_active: false`） |
| 子 agent | omp 1034551 / opencode 1049377 均已 `/exit`（`ps` 只剩一个与本次无关的、早于本轮的 omp） |
| 子 workspace | `w1S` / `w1T` 已关闭；`herdr workspace list` 只剩用户的 `wJ`（`focused=true`） |
| mini-git 冻结基线 | `scripts/check-freeze.sh` → `checked 21 file(s), drift 0`，FREEZE 未被本轮触碰 |

收尾时观察器还抓到两个事件（T1 watch seq 11 `review_due`、V1 watch seq 7 `state_changed idle`），
都是 agent 停止后的低信息量提醒，已补写回执并归为 `handled`（note 保留在 `notes/`）。
