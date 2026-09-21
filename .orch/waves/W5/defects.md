# Wave W5 缺陷与观察清单

> 范围：W5 —— 用 **更新后的 agent-orchestrator skill** 跑一个全新小项目（`/tmp/w5-linestat`，Rust CLI `linestat`），
> 目的是复验新版流程（持久 run / `jobs.py` 提交索引 / 监督断点与巡视 / 交付配置覆盖）在真实异构 agent 上是否可用。
> 轮次：T1 作者（omp）+ V1 验证者（opencode）。**W5 不是 mini-git 的开发轮**，所以本文件以**流程缺陷**为主。
>
> 编号约定：`W5-S*` = 流程/协议观察；`W5-D*` = 目标项目（linestat）产物缺陷。
> 跨 wave 合并视图见 `.orch/SKILL-FINDINGS.md`（P* 编号）与上游映射文档
> `~/Projects/agent-orchestrator/docs/2026-09-20-mini-git-current-handling-plan.md`。

## 一句话结论（截至 V1 出结果前）

新版流程在本轮**首次真正跑通**了持久 run + 提交索引 + 交付快照 + 控制方独立门禁这条链路，
并首次在**产生**（而非回放）时捕获到 `result_changed`。暴露的不是「工具不能跑」，而是
**规则只写在控制器一侧、没有下发给作者**，以及**修订语义在已发布记录上仍无正式路径**。

## 流程缺陷（编排 skill 侧）

| # | 现象 | 证据 | 对应 P / 上游状态 |
|---|---|---|---|
| **W5-S1** | **作者在发布 `result.json` 之后自行改写并重发。** T1（omp）17:45:00.997 原子发布第一版（8025 B）；随后回读自检发现报告正文手打的 `SPEC.md` sha256 错一个字符（`…cdc37d7dff…` vs 真实 `…cdc37d9ff…`），17:45:11.733 用 `os.replace` 重发修正版（8024 B）。监督器 `result_changed` 捕获到（首 pin `4e26ab4c…` vs 当前 `643efc20…`，差异 offset 268）。**制品未变**（`target/release/linestat` 仍 `75953480…`）、产品结论未变（仍 `success`），实质无害。 | `.orch/waves/W5/notes/seq8-result-changed-review.txt`；T1 watch `events/000000000008.json`；作者会话 omp jsonl（`"SPEC hash claim matches: False"`）；**作者 `prompt.txt`（含契约原文）** | **不是「规则没下发」—— 是「违反了规则，且没有补救出口」。** 复核作者实际收到的 `prompt.txt`：`protocol.py:190` 渲染的契约**逐字**写着 *"Publish atomically using a temporary file in the same directory; **never revise a valid response**."*，作者是知道的。 真正的缺口是**契约只禁止、不给出路**：没说「已经发布之后才发现事实错误该怎么办」。作者唯一正确的动作（保留原件不动、在最终回答或 follow-up 轮里声明偏差）在作者侧任何文档/模板里都没写。 **检测侧工作正常**：`result_changed` + 不可变回执把这次改动完整记录下来，这正是新版监督该做的事。→ **建议**：契约补一句「若发布后才发现事实错误，不要改写；保留原件并在最终回答 / follow-up 轮中声明该偏差」，并写明违约会被 `result_changed` 记录。严重度：🟡 中等（不是 🟠）|
| **W5-S2** | **控制方（我自己）判读交付声明时只看了 `files_created`，漏看 `files_generated`，据此误判「作者漏声明 Cargo.lock」。** 实际作者把 `Cargo.lock` 声明在 `files_generated` 下，完全合规 —— 是我的读法错了。 | `.orch/waves/W5/notes/seq7-result-review.txt`（原判）与 `notes/CORRECTION-seq7.md`（更正）；交付 manifest 的 `declaration_comparison` | 属**控制器操作错误**，非工具缺陷。但暴露一个可用性缺口：`files_created` / `files_generated` 两个近义字段并列且无说明，容易误读（`delivery.py` 报 `actual_diff` 时二者语义不同：`files_generated` 既可能是新增也可能是修改）。→ **建议**：在交付 manifest / 报告模板里紧贴字段给出「新增 vs 生成（新增或修改）」的一句话说明，或在 `delivery.py` 输出里内联该语义。 |

### W5-S1 的处置记录（按「已发布记录不追改」原则执行）

- 原始巡视记录**保留不改**（`notes/seq8-result-changed-review.txt`）；
- 结论为 `handled`，**不对目标发送任何输入**；
- 当前生效的 result（`643efc20…`）正是 `protocol.py validate` 通过、且已被交付快照固化的那一版，本轮验收不受影响。

## 目标项目缺陷（linestat）

| # | 发现者 | 现象 | 处置 |
|---|---|---|---|
| — | — | **无缺陷发现。** 控制方门禁（`delivery.py verify --snapshot … --plan plans/controller-plan.json`）**7/7 exit 0、passed: true**；独立重建的 release 制品与作者产物**哈希完全相同**；V1 自写 oracle + 36 用例逐字节差异比对在 release 制品上 **36/36 通过**；控制方再用自己构建的制品重跑 V1 的 harness，仍 **36/36**。 | — |
| — | V1（opencode） | 4 项 **SPEC 未定义**行为（`--version` 与未知选项的优先级、错误行 `<description>` 的确切文本、部分操作数失败时 total 的求和口径、空参数/`-`/`-x` 的语义）被**记录但不打分** | 视为规格留白，非实现缺陷；已写入交付配置的 `not_applicable` / coverage 说明 |
| — | V1（opencode） | 7 项**显式未覆盖**（stdin/TTY、超大文件、TOCTOU、SIGPIPE、非 Linux 路径语义等）附带原因列出 | 按要求单独报告，未被当成 pass |

## 额外方法观察

| # | 现象 | 证据 | 影响与建议 |
|---|---|---|---|
| **W5-S3** | **交付快照未使用 baseline，导致「改了什么」这一列是空的。** T1 的交付快照 `baseline: null` → `actual_diff: null`、`declaration_comparison: null`。本轮靠 `result_sha256` + 逐文件哈希 + 与 W5 基线的 SPEC 哈希比对兜住，但工具无法报告「相对开工基线新增/修改/删除了哪些文件」，也无法交叉校验作者的 `files_*` 声明是否与实际 diff 一致（只能靠 controller 人工逐个对）。 | `snapshot-nvn21yva/snapshot.json` 的 `baseline`/`actual_diff`/`declaration_comparison` 三字段 | 属**操作层缺口**：`delivery.md` 要求「先 capture 初始基线、交付时带 `--baseline` 复 capture」，本轮基线那一步没做全。→ **建议**：把「交付 capture 必须带 `--baseline`」写成交付产物的硬要求，或在 `capture` 不带 baseline 时给出显式警告，避免 `null` 被静默当成「无变更」。 |
| **W5-S4** | **`usage.py` 的 `purpose` 词表仍缺 `verify`**（P19 原样）：验证轮的用量只能记成 `task`，「按角色/轮型拆成本」在数据模型层仍做不到。 | `usage.py` 的 `PURPOSES`；W5 两个 sample 都只能填 `task` | P19 再现，非新问题；本轮的角色分列是人工的。 |

## 新版流程在本轮**首次生效**的能力（正面对照，详见 summary.md）

| 能力 | 本轮实测 |
|---|---|
| herdr 字母型资源 ID（P1） | T1 拿到 `w1S` / V1 拿到 `w1T`，`protocol.py` 校验通过 —— 旧版会直接拒收 |
| 持久 run + 提交索引（`jobs.py` begin/receipt） | 全程 `orch-run-2lpx81iy`，每条 mutation 带 `--token` + `--expect-revision`，revision 单调递增 |
| 交付快照 + 控制方独立门禁 | `snapshot-nvn21yva` 封存 6 文件；控制方在**独立构建目录**重建 release 制品并比对哈希 |
| `result_changed` 监督（新事件类型） | 在**真实产生**时捕获 W5-S1（W1–W4 从未有过这类事件） |
| 交付配置覆盖（P23/A 批） | 任务包按 dev / release / release-artifact 分列，作者与验证者都被要求逐配置给结论，不得只报测试总数 |
| 监督巡视事件 / 断点 | T1 watch 10 个事件、V1 watch 4 个事件，全部按 `--expected-revision` 写不可变回执 |

## 上游处理结果（2026-09-20 18:12，`b124b87`）

上游读了 `.orch/upstream/2026-09-20-w5-upstream-issues.md` 并逐条处理（见上游
`docs/2026-09-20-w5-follow-up.md`）；运行包已同步到 vault `cc1a069`，本机运行时即最新。
逐条核对见 `.orch/SKILL-FINDINGS.md` §0.3。

| 本文件编号 | 上游编号 | 处理结果 | 我的本地实测 |
|---|---|---|---|
| **W5-S3** | U1 | ✅ 已修：`capture` 返回值与封存 manifest 增加稳定 code 的 `warnings`（`no_baseline`/`no_result`）；交付流程明确 `--baseline` 必填 | 不带 baseline → `[{code:no_baseline}]`；带 baseline → `[]` 且 diff/声明比对均非 null；W5 旧快照仍可读 |
| **W5-S1** | U2 | ✅ 已修：契约串 + author/rework 模板 + 受控说明三处补齐「保留原件/正常输出声明/等控制器新轮次」 | 三处文案命中；受控说明的行文正是我们这次「哈希错一位」的场景 |
| **W5-S2** | U5 | ✅ 已修（文案）：`delivery.md` 改成四行类别对照表 + 「四个列表都要读」 | 文档已就位（代码分类本来就正确） |
| **W5-S4** | U4 | ✅ 后续批次完成（kumi `a58905d`）：新增与 purpose 正交的 `role`/`phase` 维度；同时补 OpenCode/Hermes 原生适配（P15），并提供 `usage.py correct` 做带证据的原子更正 | V1 样本已更正为 `role=verifier / phase=verification`；**并借此发现我 W5 账本里一条真实错误**：output 记成 16,776，漏加了 reasoning 17,796，已更正为 34,572 |

### 附：本批暴露的我们这侧的缺口

上游新要求「验证必须同时测已知正确与故意错误」。回头核对发现，**W5 收口时我验收 V1 只跑了正例**：
如果 V1 的 harness 是「永远 PASS」的假绿，V1 和我的两次 36/36 就都毫无判别力（P18 的老问题）。
已按新配方补做独立 attempt 作为补充证据（`evidence/v1-acceptance-negative-control.md`）：
故意答错的替身 36/36 全被拒、真制品 36/36 通过。原验收记录按「已发布记录不追改」保持原样。
