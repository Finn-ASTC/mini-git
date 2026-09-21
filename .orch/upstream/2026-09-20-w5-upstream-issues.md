# 给 agent-orchestrator 的 issue 清单（来自 mini-git W5 复验轮）

日期：2026-09-20。来源：用**更新后**的 skill 跑的一个真实小项目（`/tmp/w5-linestat`，Rust CLI，2 个异构 agent 轮：
作者 omp 18.2.6 + 验证者 opencode 1.18.29），全程 11 分钟、零返工。
本轮 skill 版本：本机 vault 运行时（`~/.agents/skills/agent-orchestrator`），与上游 `agent-orchestrator` 开发树逐文件一致。

**背景**：本轮的目的是复验上游 A 批（交付配置）与新增的持久 run / `jobs.py` / `supervision.py` 是否真的可用。
结论是**可用**，并且比 W1–W4 少踩了很多坑（字母资源 ID、提交索引、交付快照、`result_changed` 监督、
逐配置覆盖、`check-completion` 硬门禁都实测生效）。下面 5 条是本轮**新发现或仍未处理**的问题，按建议优先级排列。

现场证据（全部在本仓库内，可直接引用）：
`.orch/waves/W5/summary.md`（全过程 + 成本）、`.orch/waves/W5/defects.md`（W5-S1~S4）、
`.orch/waves/W5/evidence/`（验收与 host 观察）、`.orch/waves/W5/notes/`（巡视回执与更正）、
`.orch/SKILL-FINDINGS.md` §0.1（P1–P25 逐条现状）。

---

## 1. `delivery.py capture` 缺 `--baseline` 时静默丢掉整个「改了什么」维度（建议优先）

**严重度**：中高。**类型**：工具行为（静默降级）。

**现象**：交付捕获不带 `--baseline` 时，manifest 里 `baseline` / `actual_diff` / `declaration_comparison`
**全部是 `null`**，而 `capture` 的返回和 stdout **没有任何警告**，字段看起来和正常输出一样。
于是「相对开工基线增删改了哪些文件」这一整列**凭空消失**，作者 `files_*` 声明也无法与实际 diff 交叉校验
（`declaration_comparison` 的四个分类 `undeclared_changes` / `declared_without_scoped_change` /
`outside_snapshot_scope` / `category_mismatches` 一并失效）。

**本轮实证**：T1 的交付快照 `snapshot-nvn21yva/snapshot.json` 三个字段均为 `null`；
本轮靠 `result_sha256` + 逐文件哈希 + 与 wave 基线比对 SPEC 哈希兜住，但那是人工补的。

**代码位置**：`scripts/delivery.py:187`（`if baseline is not None`）、`:210`（`comparison = None`）、
`:234-235`（`baseline` / `actual_diff` / `declaration_comparison` 三字段条件写入）；文档 `references/delivery.md:52-58` 只说明「With `--baseline`, ... Without a baseline,
actual diff remains null」，读者容易把它当成可选增强而不是证据缺口。

**建议**：
1. `capture` 在 `baseline is None` 时在返回值和 manifest 里给出**显式告警字段**（例如
   `warnings: ["no_baseline: actual_diff and declaration_comparison are unavailable"]`），不要让 `null` 静默通过；
2. 在 `references/delivery.md` 的「交付」步骤里把 `--baseline` 写成交付捕获的**硬要求**
   （初始基线捕获那一节已经要求了，但两节之间没有强绑定）；
3. 若确实允许无基线捕获（例如只做存在性/哈希封存），文档明说此时**不得**把 `declaration_comparison`
   当作「已核对」的证据。

**验收标准**：不带 `--baseline` 的 `capture` 有可机器读取的告警；带 `--baseline` 时四个 diff 分类都非 null。

---

## 2. 作者契约禁止改写已发布结果，但没给「发现已发布内容有错」的出口

**严重度**：中（本轮无害，但行为不确定）。**类型**：契约/文档。

**先纠正我自己**：我最初把这条写成「规则只写在控制器一侧、没下发给作者」，**这是错的**。
复核作者实际收到的 `prompt.txt` 后确认，`scripts/protocol.py:190` 渲染的契约里**逐字**写着：

> Publish atomically using a temporary file in the same directory; **never revise a valid response**.

所以作者是**知道**这条规则的 —— 问题在于它**违反了**规则，而契约**只禁止、不给出路**。

**本轮实证**：T1（omp）17:45:00.997 原子发布第一版 `result.json`；随后**回读自检**发现报告正文里手打的
`SPEC.md` sha256 错了一个字符，于是 17:45:11.733 用 `os.replace` 直接重发。
监督器正确报出 `result_changed`（首 pin `4e26ab4c…` vs 当前 `643efc20…`，差异在 offset 268）——
**检测侧工作正常**，这正是新版监督该做的事。制品与结论均未变，实质无害。

**缺口**：作者发现「已经发布的结果里有事实错误」时，唯一正确的动作是
**保留原件不动 + 在最终回答或 follow-up 轮中声明该偏差**，但这句话在作者侧任何地方都没写：
`references/task-workflow.md` 全文 **0 次** 命中 `result.json` / `publish` / `correct`；
`references/delivery.md` 只在 `:135` 提「source corrections require a new delivery snapshot」，
说的是源码更正而不是报告正文更正。控制器侧 `references/supervision.md`
（§"Published results must remain unchanged"）倒是有完整交代，但**作者看不到那一页**。

**建议**：在 `protocol.py` 的契约串里，紧接 "never revise a valid response" 补一句出口，例如：

> If you notice a factual error in a response you already published, do not rewrite it;
> retain the original and state the correction in your final answer or the next round.

并在 author/rework 模板（`assets/task-packets/`、`references/task-workflow.md`）里重复一次。
可选：说明违约会被 `result_changed` 记录，因此「静默重发」不会真的静默。

**验收标准**：作者侧文档/契约出现「已发布后发现错误」的明确处置；回归测试里有一条断言该句子存在。

---

## 3. 验证轮自己的产物怎么验收，文档没有路径

**严重度**：中低（可绕开，但每个用的人都要重新发明一遍）。**类型**：文档缺口。

**现象**：`completion.md` 的 `accepted_by_verifier` 是**作者 job 上的一个事实**，
但没有任何一处说明**验证轮自己的 job**该怎么收口。实际用 `jobs.py accept` 时会发现：
`kind: "answer"` 不适用（验证轮会产生文件），`kind: "delivery"` 又**必须**提供
`attempt_path`，而那个 attempt 只能来自 `delivery.py`。

**本轮我是这么解开的**（可作为文档示例）：
1. 对验证者的证据范围做一次 `delivery.py capture`（`--include REPORT.md --include oracle --include harness
   --include evidence`，`--exclude work`，`--root` 指向 run 目录外的存储）；
   manifest 的 `result_sha256` 自动绑定这次发布的 result；
2. 写一份 `verify` plan，作用不是「重读它的报告」，而是**用控制方自己构建的制品重跑验证者自写的 oracle**
   （并先断言 oracle / harness / 制品三者的哈希）；
3. 用该 attempt 走 `jobs.py accept --input`（`kind: delivery`）。

**建议**：在 `references/completion.md` 的「Independent acceptance」一节补一小段
「accepting a verification round」，把上面三步作为示例写明；
并说明关键原则：**验收验证轮时，重跑的是它的 oracle，不是重读它的结论**。

**验收标准**：文档给出验证轮 job 的 `accept` 最小示例；示例中的 evidence/attempt 绑定关系可被工具校验。

---

## 4. `usage.py` 的 `purpose` 仍无 `verify` / `rework`（P19，仍未处理）

**严重度**：中（数据模型层缺陷，无绕开）。**类型**：功能缺失。

**现象**：`scripts/usage.py:17` 的 `PURPOSES` 仍是 `("task", "retry", "report_repair")`。
验证轮与返工轮只能记成 `task`，「按角色/轮型拆成本」在数据模型层做不到 ——
`summary` 的 `attempts` 分列永远是 `task: N`。

**本轮实证**：W5 两个轮次（作者 T1、验证者 V1）都只能以 `purpose: "task"` 记录，
角色分列是我**人工**做的（`.orch/waves/W5/summary.md` §4）。

**建议**：把 `verify` 加入 `PURPOSES`；若担心向后兼容，读侧把未知 purpose 显示为 `unknown` 而不是报错。
配套把 `references/efficiency.md:157` 的词表说明一起改。

**验收标准**：一个含 author / verify / rework 三轮的夹具，`summary` 的 `attempts` 能分别计数。

---

## 5. `files_generated` 的散文说明容易被读成「另外三个字段是模糊的」（低）

**严重度**：低（**本轮是我自己读错**，先记下来但不必急）。**类型**：文档措辞。

**现象**：`references/delivery.md:57` 写「`files_generated` may represent an addition or modification」。
单独看这句，很容易推论出「四个字段都可能不精确」，从而在核对声明时去找一个并不存在的模糊。
实际上**代码是精确的**：`scripts/delivery.py:218-227` 的 `category_mismatches`（`:221`） 里，
`files_created` / `files_modified` / `files_deleted` 各自**只**匹配 `added` / `modified` / `deleted`，
**只有** `files_generated` 匹配 `added + modified`。作者侧契约（`protocol.py:186-188`）也写清了四个列表的语义。

**本轮实证**：我在 `notes/seq7-result-review.txt` 里据此误判「作者漏声明 `Cargo.lock`」，
实际作者把它放在 `files_generated` 下完全合规；已按「已发布记录不追改」另立
`notes/CORRECTION-seq7.md` 更正。

**建议**：把 `delivery.md:57` 那句改成对称表述，例如：

> `files_created`, `files_modified` and `files_deleted` must each match their own diff category exactly;
> only `files_generated` spans additions and modifications.

**验收标准**：文档不再让读者以为 `files_created` 也可能是修改。

---

## 附：本轮**已验证可用**、无需改动的能力（避免上游重复排查）

| 能力 | 本轮实测 |
|---|---|
| 字母型 herdr 资源 ID（P1） | `w1S` / `w1T` 全程通过 `protocol.py` 校验；`cleanup-plan` 生成的清理动作精确到 `owns_workspace` |
| 持久 run + `jobs.py` 提交索引 | 18 次 mutation，`--token` + `--expect-revision` 全通过，revision 单调递增，无冲突 |
| 交付快照 + 控制方独立门禁 | `capture` → `verify` 两次都成功；控制方独立重建的 release 制品与作者产物**哈希完全相同** |
| `result_changed` 监督 | **首次在真实产生时**捕获（issue 2 的场景），回执链完整 |
| 交付配置覆盖（P23 / A 批） | dev / release / release-artifact 三配置逐条出结论；「构建成功 ≠ 行为检查」被两边都遵守 |
| `check-completion` 硬门禁 | 缺 `acceptance` / `host` 时明确拒绝（`ready_to_complete: false` + 原因），无法只凭「作者说通过」收口 |
| `supervision` 巡视与断点 | 18 个事件全部有不可变回执；`runs.py recover` 终态 `complete: true, action_required: 0` |

---

# 上游处理结果（2026-09-20 18:12 之后核对）

上游读了本清单并逐条处理，记录见上游 `docs/2026-09-20-w5-follow-up.md`。
开发仓库 `b124b87`（18:12）→ 运行包 vault `cc1a069`；**本机运行时已同步**
（6 个 skill 与开发树 `diff -rq` 零差异，排除 `__pycache__`；vault `PROVENANCE.md` 已记来源 `b124b87`）。
上游自报 **313/313 通过，含 13 项真实 tmux**。

| 编号 | 我们的发现 | 上游处理 | 我的本地复核（在**更新后的运行时**上实跑） |
|---|---|---|---|
| **U1 = W5-S3** | `capture` 缺 `--baseline` 静默丢掉整个 diff 维度 | `capture` 返回值**和**封存 manifest 都增加稳定 code 的 `warnings`（`no_baseline` / `no_result`）；`delivery.md` 明确「要审差异/声明的交付，`--baseline` 是本流程的必填」；保留初始快照用途与旧版读取 | ✅ 实测：不带 baseline → `warnings: [{"code":"no_baseline",...}]`，返回与 manifest 完全一致；带 baseline → `warnings: []` 且 `actual_diff` 与 `declaration_comparison` 都非 null；W5 旧快照（无 `warnings` 字段）`delivery.py check` 仍 `valid: true` |
| **U2 = W5-S1 = P25** | 作者契约禁止改写已发布结果，但没有「发现错误后怎么办」的出口 | 三处补齐：① `protocol.py` 契约串补「保留原件、在正常输出里声明更正、停下等控制器给新轮次/path，通知不能替代响应」；② author/rework 模板各加一条同样的约束；③ agent-controlled `protocol.md` 补受控说明，**明确「一个字符的哈希笔误也不允许替换已发布响应」** | ✅ 实测：`grep` 到契约句与两个模板句；受控说明 §109-122 的行文**正是我们 W5 的场景**（作者回读自检发现哈希错一位） |
| **U3** | 验证轮自己的产物怎么验收，文档没有路径 | `completion.md` 新增 §"Accepting a verification round"：可执行 JSON 计划示例 + **必须同时测已知正确与故意错误** + 明确「拒绝非零退出可能就是缺依赖」+ 「借用作者的 attempt 不能算」 | ✅ 实测：按该配方对 W5 的 V1 快照补跑 —— 故意答错的替身 **36/36 全被拒**（exit≠0），真制品 **36/36 通过**，`passed: true`。见 `.orch/waves/W5/evidence/v1-acceptance-negative-control.md` |
| **U4 = P19** | `usage.py` 的 `purpose` 仍无 `verify`/`rework` | ✅ **后续批次已完成**（kumi `a58905d` 起）：没有往 `purpose` 里塞词，而是新增**与 purpose 正交的 `role`/`phase` 维度**，`summary` 输出分组；同时补上 OpenCode/Hermes 原生适配（P15）。上游新增 `usage.py correct`（原子追加、保留原样本） | ✅ 已用新工具更正了本轮账本里被我算错的一条：V1 的 output **16,776 → 34,572**（漏加 reasoning），并补 `role=verifier / phase=verification`。证据 `evidence/v1-opencode-native-usage.json`；逐条核对见 `.orch/SKILL-FINDINGS.md` §0.4 |
| **U5 = W5-S2** | 文件类别表述不对称，容易被读成「四个字段都模糊」 | `delivery.md` 改成四行对照表（`created`→`added`、`modified`→`modified`、`deleted`→`deleted`、**只有** `generated` 跨 `added`/`modified`），并加一句「四个列表都要读；新生成的 lockfile 只出现在 `files_generated` 不等于没声明」 | ✅ 文档表格已就位；代码分类本来就正确（`delivery.py` 的 `category_mismatches` 语义未变） |

## 本轮顺带暴露的**我们这边**的缺口

上游新增的负对照要求让我发现：**W5 收口时我验收 V1 只跑了正例** ——
如果 V1 的 harness 是「永远 PASS」的假绿，我和它的 36/36 就都没有意义（P18 的老问题）。
已按新配方补做独立 attempt 作为补充证据（见上表 U3 行的链接）；
原验收记录按「已发布记录不追改」保持原样，未追溯修改 `jobs.py` 的已完成关闭。
