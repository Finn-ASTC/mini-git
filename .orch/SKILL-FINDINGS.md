# agent-orchestrator 方案问题记录（持续更新）

> **这是什么**：把 `~/.agents/skills/agent-*`（agent-orchestrator / agent-controlled /
> agent-omp / agent-hermes / agent-opencode / agent-codex 六件套）当作编排方案使用过程中，
> 实际踩到的**问题、坑与结论**。每条都带现象、证据、最小复现、影响、建议处置。
>
> **记录规则**：只记**实测**到的行为，不写推测；每条必须能在本机复现。
> 逐 wave 的证据留在 `.orch/waves/<W>/defects.md`，本文件是**跨 wave 的合并视图**。
>
> **给上游的 issue 清单**：`.orch/upstream/2026-09-20-w5-upstream-issues.md`
> （W5 复验轮新发现 5 条 + 本轮已验证可用的能力表，按建议优先级排列，含可复现的证据路径与验收标准）。
>
> **维护者**：controller（本项目里是主 agent / 人）。
> 最近更新：**上游 C1/C2/B2 批（kumi-public `d90cc8d`）后的第五次核查**（见 §0.5）：新增 P26–P29（1 🟡 / 3 🟢）；
> 同时清掉了上一轮「vault 比 dev 仓库新」的疑问 —— 私有检出 `~/Projects/kumi` 已归档，**kumi-public 是权威源**，
> 运行包 `diff -rq` 零差异。
> 上一次更新：**上游用量适配批 + 项目改名 kumi 后的第四次核查**（见 §0.4）：P15 ✅、P19 🟢、P16 改善，25 条已无「未处理」项。
> 上一次更新：**上游 W5 跟进批（`b124b87`）后的第三次核查**（见 §0.3）。
> 上一次更新：**W5 复验轮 + §0.1 第二次核查**（2026-09-20；W5 用更新后的 skill 跑了真实异构两轮，新增 P25，并把 P3/P4/P13/P16/P23 的现状按上游文档边界与实跑证据重新定级）。
> 上一次更新：**上游现状核查**（2026-09-20，P1–P24 逐条标注上游是否已修；见 §0.1 / §0.2）。
> 上一次更新：**W4 收口 / v1 完成**（2026-09-19，P1–P22 全部成文；证据归档见 `.orch/artifacts/`）。
>
> **重要前提**：本文件里提到的 skill 代码**故意没有修改**（除了明确标注"已修"的项）。
> 因为本测试床的目标是测量这套方案的真实能力边界，中途打补丁会让各 wave 失去可比性。

> **「上游现状」标注怎么读**（2026-09-20 核查）：每条 P 的标题下多了一行 `上游现状`，取值：
> ✅ **已修**（上游改了代码，本机实测行为已变）／🟢 **已覆盖**（写进了正式协议、模板或配方）／
> 🟡 **部分缓解**（根因仍在，但有了正式的绕开或降级做法）／🔴 **未处理**（上游原样）／
> ⬜ **不适用**（属测试床或宿主侧，上游不覆盖）。
> **正文一律保持原样** —— 正文是当时的实测记录，不追改；「上游是否已修」只写在标注行里。

> **原始证据在哪**（2026-09-19 W4 收口时从 `/tmp` 抢救归档）：
> `.orch/artifacts/` —— controller 自建工具（`tools/`）、codex 审批逐条日志（`logs/`）、
> token 清单（`usage-manifests/`）、开工前的 skill 自检产物（`preflight/`）、
> 验证者对拍 harness 与变异体（`harnesses/`、`mutants/`）。
> 索引与来源见 `.orch/artifacts/README.md` 与 `PROVENANCE.md`。

---

## 0. 问题总览

> 下表是**项目期间（W1–W4）的实测结论**，按「不追改历史」的原则保持原样 ——
> 表里写「未修」只代表**当时**没修，**上游是否已修一律看 §0.1**。

| # | 严重度 | 一句话 | 影响面 | 状态（项目期间）|
|---|---|---|---|---|
| P1 | 🔴 **阻塞** | insider（可见）模式下 `record` / `watch` / `cleanup-plan` 三个命令全部不可用 | 整条「在用户会话里开可见子 agent」的链路 | 未修，用变通绕过 |
| P2 | 🔴 阻塞（流程） | follow-up 轮次的 `prepare --previous` 被同一校验阻断 | 返工/追问链路 | 未修，改用全新 prepare |
| P3 | 🟠 严重 | hermes 每轮结束后自动改**私有 skill**，属协议外持久副作用且 result 里看不见 | 写作用域可信度、成本 | 未修，已知需固定检查 |
| P4 | 🟠 严重 | `agent start` 在**既存会话**里偶发超时（命令已输入未执行）；重跑会重复输入 | 启动可靠性 | 有文档化配方，1 次恢复成功 |
| P5 | 🟠 严重（成本） | codex 对复合 bash **逐条**弹原生审批，是唯一的吞吐瓶颈 | 单轮耗时可翻倍 | 未修，按纪律只批单条 |
| P6 | 🟡 中等 | `watch.py` 对持续重绘 TUI 产出大量 `review_due` 噪声 | 监督成本 | 未修，降级为采样 |
| P7 | 🟡 中等 | `agent get` 的状态与实际执行**不可靠**，且跨 kind 表现不一 | 判定依据 | 已改用 pane + 结果文件判定 |
| P8 | 🟡 中等 | 轮次结束后仍可能弹**产品级**对话框（omp 的"学习练习"），任务包禁止也挡不住 | 监督必须常驻 | 未修，现场处理 |
| P9 | 🟡 中等 | 共享 checkout + 共享 `target/` 会互相污染（编译噪声、cargo 缓存被同名 crate 覆盖） | 并行正确性 | 部分写进任务模板 |
| P10 | 🟢 轻微 | 任务包里的**规格错误**只能靠 agent 用真值 oracle 发现 | 规格质量 | 已建立「任务包原样保留 + 决策记录修正」的做法 |
| P11 | 🟠 严重（规格） | **任务书自己指定的「真值来源」也可能是错的**，验证者若照抄就会造出假绿 | 验收可信度 | W2 两处实例，已改为「任务书只给真值意图，来源由验证者自证」 |
| P12 | 🟡 中等 | `prepare --parent-depth` 的语义与任务书里写死的 "depth" 数字不一致，controller 必然试错一次 | 协议易用性 | 未修，记录期望语义 |
| P13 | 🟡 中等 | 并发 wave 里**改动归因只能靠 mtime 窗口**，协议本身没有「谁改了哪个文件」的机制 | 越界判定可信度 | 未修，靠 `drift.sh` + mtime 时间窗人工归因 |
| P14 | 🟢 轻微 | `agent prompt` 返回的 `agent_status` 恒等于调用前的状态（本次 5/5 实测都为 `idle`），不能当开工信号 | 监督判定 | 已改用 revision 增长 + 结果文件 |
| P15 | 🟡 中等（可降级） | `usage.py` 的原生导入只支持 **codex/omp**；hermes/opencode 没有适配器，而面板读数是**上下文占用**而不是累计用量 → 跨 kind 成本对比不成立 | 成本指标 | W4 收口实测：hermes(`~/.hermes/state.db::session_model_usage`) 与 opencode(`~/.local/share/opencode/opencode.db::message.data.tokens`) **都有完整 usage 数据**，是「缺适配器」不是「缺数据」；四个 host 的全项目总账已核出（`waves/W4/summary.md` §4.1）|
| P16 | 🟢 轻微 | 原生导入必须**逐轮手工**建立 `native session ↔ round` 映射（manifest 里要写死 log_path/session_id/native_id），没有「按 pane/时间自动归属」的路径 | 成本采集成本 | 未修，W2 的 6 个 manifest 存在 `.orch/artifacts/usage-manifests/` |
| P17 | 🟠 严重（流程） | **门禁测试的所有权没有定义**：随实现推进，早先 wave 的测试会「断言旧世界」而变红；child 按写作用域纪律只能报 blocked，controller 必须准备接住 | 长链条编排的门禁与返工判定 | W3 出现 3 例（smoke / verify_worktree / V11 的 characterization），controller 用 C-18/C-19 + V11b 处理 |
| P18 | 🔴 **阻塞（可信度）** | **「真值」缺夹具就没有真值**：同一句「真实 git 会 X」在夹具不同的两格里结论相反（symlink 祖先 + 目标 rev 修改：顺 symlink **stat 得到** → exit 1 拒绝；**stat 不到** → exit 0 并换真目录）。验证者只报结论不报夹具 → controller 复跑时换了变量、得出相反结论、把「验证者测错」写进返工任务书 → 同一错误连发两次 | 验收可信度、返工成本、controller 的仲裁可信度 | W3 实例：V11 的 FAIL 2 与 controller 的 v1 更正**两边都是实测正确的**，差别只在 `outside/` 空不空；作者 T11b 把夹具做成 42 格矩阵才终结争议，改码后 42/42 与 git 一致 |
| P19 | 🟡 中等 | `usage.py` 的 `PURPOSES` 只有 `task/retry/report_repair`，**没有 `verify`/`rework` 词表**：验证轮与返工轮在成本账本里只能记成 `task`，「按角色/轮型拆成本」这件事在数据模型层就做不到 | 成本指标、返工成本核算 | W3 实测（6 个 manifest 被迫全填 `task`）；好消息：同一 native session 的续轮是两个 `turn`，可按 `native_id` 精确切分（V11/V11b 已分开计量） |
| P20 | 🟡 中等（流程） | **过期的 `#[ignore]` 标记不会自己失效**：缺陷被修好后标记仍在，用例**从绿色里静默消失**（W2 的 V6 标记一直挂到 W4 才被发现；W4 另有 3 处） | 覆盖率可信度、「回归用例数量」这类指标 | W4 controller 复跑 `cargo test --offline -- --ignored` 对账：4 处已恒绿、1 处仍是已确认的已知分歧；已把「wave 收口复跑 `--ignored` 逐条对账」写进流程（C-25） |
| P21 | 🟠 严重（可信度） | **并发 wave 里，作者观测到的「peer 的缺陷」可能是中间态**：共享 checkout 下没有人知道「我 build 的是哪一版制品」，于是把飞行中的半成品报成缺陷 | 缺陷归因、返工判定、跨任务协作的可信度 | W4 实例：T15（21:54 完成）报 `mg push` 拒绝 bare 远端/新建分支；V13 复核（22:21+）时 T13 的最终交付里两者都工作 —— 归因修正见 `.orch/waves/W4/defects.md` W4-D3 / W4-S3 |
| P22 | 🟢 轻微（流程） | **任务包的「门禁」一节没有 `cargo fmt`**：agent 只被要求跑 `clippy`/`test`，格式债务在 4 个 wave 里一路累积（收口时 47 个文件需要重排）| 交付物整洁度；收口成本 | W4 收口执行 `cargo fmt --all`（C-29），FREEZE → v0.7；无任何接口/行为改动 |
| P23 | 🟠 严重（流程） | **门禁只在 debug profile 下跑**：所有任务书/验证任务书的门禁都是 `cargo test --offline`，4 个 wave 40 轮**没有一次** `cargo test --release`。于是「`debug_assert!` 里的副作用在 release 下被编译掉」这类缺陷完全不可见 —— W4 收口后补测发现 `mg` 的 **release 构建会死循环、且输出与 git 不一致** | 发布构建的正确性、门禁的可信度 | W4 收口后实测：release 下 2 个 target 卡死（lib / verify_diff），200 例随机语料中 **3 例死循环 + 12 例输出与 git 不同**；根因 8 处 `debug_assert!(带副作用的调用)`（`src/diff/myers.rs`）。见 `.orch/waves/W4/defects.md` W4-D12 / W4-S12 |
| P24 | 🔴 **阻塞** | **`debug_assert!` 里带副作用的调用在 release 下被整条编译掉**：`src/diff/myers.rs::change_compact` 有 8 处断言包住了会改 `&mut` 状态的 `group_previous` / `group_next` / `group_slide_up` → release 下 `while` 变空循环，`mg diff` 与 `mg merge` 三方路径 **100% CPU 永久挂起**，输出也不对；debug 完全正常 | 发布构建不可用 | **未修**（用户裁决「暂时不修」）；现场归档与最小复现见 `.orch/artifacts/release-p24/`；流程教训见 P23（W4-D12 / W4-S12）|
| P25 | 🟡 中等（流程） | **「已发布结果不得改写」这条规则确实下发给作者了 —— 但作者违反了它，而契约里没有「事后发现错误该怎么办」的出口。** `protocol.py:190` 渲染的作者契约（在 W5 的 `prompt.txt` 里逐字可见）写着 *"Publish atomically using a temporary file in the same directory; **never revise a valid response**."*，所以这不是「作者不知道规则」。W5 实测：作者发布后回读自检发现报告里手打的 SPEC 哈希错一位，**直接 `os.replace` 重发**；监督器正确报出 `result_changed`（检测侧工作正常）。缺口在于：契约只说「不许改写」，没说**「已经改不了的情况下怎么把错误交付出去」** —— 作者唯一正确的动作（保留原件不动、在最终回答或 follow-up 轮里声明该偏差）在作者侧任何文档里都没写 | 已发布记录的不可变性；作者遇到「已发布结果有事实错误」时的行为不确定 | 2026-09-20 **W5 实测**（2 轮真实异构 agent，制品与结论未变、实质无害）。建议：契约补一句「若发布后才发现事实错误，不要改写；保留原件并在最终回答 / follow-up 轮中声明该偏差」，并说明违约会被 `result_changed` 记录 |

## 0.1 上游现状核查总表（2026-09-20）

skill 已更新，本文 25 条逐条核对了一遍。**结论（2026-09-20 第四次核查后）：4 条已修、12 条已进入正式流程、8 条部分缓解、0 条上游仍未处理、1 条不适用。**
> **2026-09-21 第五次核查**：上游 C1/C2/B2 批（kumi-public `d90cc8d`）**没有回退任何既有处理**；本轮新发现 P26–P29，其中 **P26 🔴 未处理**（delta 的 reset 路径无条件重写内容缓存，见 §0.5），P27–P29 为 🟢 文档/可用性。
> 2026-09-20 第二次核查（**W5 实跑后复核**）：P3 / P4 / P13 / P16 从「已覆盖」下调为「部分缓解」，P23 从「不覆盖」上调为「已覆盖」。
> 调整依据是上游自己文档里的边界声明（如 `delivery.md` 明说 manifest **不证明写者**、`isolation.md` 明说私有 home **不是 OS 沙箱**）以及 W5 的真实运行观察，不是本轮新出现的代码变更。
> **更正记录**：P25 初稿写成「规则只写在控制器一侧、没下发给作者」，复核作者 `prompt.txt` 后确认该说法**不成立**（`protocol.py:190` 已逐字下发），已改为「规则已下发、缺事后出口」，严重度由 🟠 降为 🟡。
> **2026-09-20 18:12 第三次核查**：上游 `b124b87` 处理了 W5 复验轮提出的 U1–U3/U5，P25 → 🟢。逐条核对见 §0.3。
> **2026-09-20 22:50 第四次核查**：上游后续 8 个提交补上用量适配与独立 role/phase —— **P15 → ✅、P19 → 🟢、P16 改善**，25 条**已无「未处理」项**；项目同时改名为 kumi。见 §0.4。

| # | 上游现状 | 上游依据（文件 / 上游跟进项）|
|---|---|---|
| P1 | ✅ 已修 | `protocol.py:24` 改用字母型 ID 字符集；F01（vault sync `135d44a`）；**本机实测 `record` / `cleanup-plan` 通过**。**W5 再次实测**：两个子 agent 拿到 `w1S` / `w1T` 字母型 workspace ID，`protocol.py` 全程校验通过，收尾用工具生成的 `cleanup-plan` 拆掉，父 pane 未受影响 |
| P2 | ✅ 已修 | `protocol.py:231-235` follow-up 继承 cwd/depth 并拒绝覆盖；**本机实测 `prepare --previous` 通过** |
| P3 | 🟡 部分缓解 | `agent-hermes/references/isolation.md` 改为「新任务优先私有 home，续轮复用原 home」，并明确 **HERMES_HOME 不是 OS 沙箱**；上游同页结论「私有存储不提供 OS 隔离，不保证任意后台活动都能发现」。宿主侧自动维护行为未变。**W5 补充**：本轮没跑 hermes，但 insider 模式下 **omp 确实写到真实 `~/.omp`**（session 日志、`logs/`），说明「私有存储」只在 hermes 侧提供；W5 已把这些路径写进 host 观察的 `side_effect_paths` |
| P4 | 🟡 部分缓解 | `transports.md:14` 正式化为「startup failure 先记录/清理部分资源；可能已被接受时先 recover 既有 job，再考虑换 transport，不允许自动重跑」。**流程与 runner 边界已实现，但新 runner 的「四宿主实际启动恢复」仍待测** —— 上游 2026-09-20 报告只覆盖了限定的原生故障样本 |
| P5 | 🟡 部分缓解 | `agent-codex/SKILL.md` 明确不关审批；`lifecycle.md:46/52` 同一问题只中继一次；**宿主逐条弹审批未变** |
| P6 | ✅ 已修 | `efficiency.md:84/102` 退避 60→300s + 同类提醒分组；F04（`a84afff`）|
| P7 | 🟡 部分缓解 | `jobs.py begin/receipt` + SKILL.md「CLI success alone is not confirmed acceptance」；跨 kind 状态语义仍未定义 |
| P8 | 🟢 已覆盖 | `agent-omp/references/lifecycle.md:12/47` 产品提问单独处置并记为 waiting/unknown；F06 |
| P9 | 🟢 已覆盖 | `references/delivery.md` + `scripts/delivery.py`（独立 build/cache/evidence）；F05 |
| P10 | 🟡 部分缓解 | `task-workflow.md` 规定冲突时返回 blocked + 最小反例；**发现规格错误仍靠真值 oracle** |
| P11 | 🟢 已覆盖 | `verify.json` 钉死 oracle 版本/夹具哈希 + 「先用已知错误输出试 oracle 必须被拒」；F09 |
| P12 | 🟡 部分缓解 | follow-up 自动继承（同 P2）；错误信息给出行动指引；**首次 `prepare` 仍需显式 `--parent-depth`** |
| P13 | 🟡 部分缓解 | delivery manifest（scope + handoff + 逐条哈希 + request digest）把归因从 mtime 窗口提升到「快照绑定」，F05。但 `delivery.md:56` 自己写明 **"It does not infer the writer or prove attribution"**，且 `snapshot` 的 `stability` 只声明「协作写者的**一致性**检查，不是原子快照，也不防敌意写者」。真正的写者归因/OS 隔离仍是独立工程。**W5 实测**：V1 确实**自己发现并界定了**「seal 之后快照目录里多了一个 `verifications/`，是外部行为者、不是我」，说明快照纪律在真实运行里能支撑这种判断 —— 但那是靠**人/agent 读 manifest**，不是工具给出的归因 |
| P14 | 🟢 已覆盖 | `jobs.py` 证据化 receipt；`efficiency.md:78`「state is not round success」；`runs.md:141` |
| P15 | ✅ 已修 | `native_usage.py` 现在支持**四种宿主**：`--host {codex,omp,opencode,hermes}`。OpenCode 走新增的 `native_stores.read_opencode`（SQLite/export，按 assistant message ID 去重、按 parent user message 映射轮次，`input = input + cache.read + cache.write`、`output = output + reasoning`）；Hermes 走 `hermes_hooks.read_log`（累计快照，明确拒绝当作 round delta）。**本机实测**：`inspect-native --host opencode --session-id ses_0004…` 解析出 24 个 assistant call，input 1,510,971 / cached 1,440,896 / output 34,572 |
| P16 | 🟡 部分缓解 | 新增 `usage.py discover-sources --run`（列出 run 内 controller 与每个 round 的 native 候选及已记来源，**只发现不绑定**）、`audit-sources`（审计显式绑定与用量缺口）、`project-summary`（跨 job 去重汇总）。**本机实测**：对 W5 的 run 跑 `discover-sources` 能正确列出两个 round 的原生 session 候选与 ledger 来源。但绑定仍是显式的，且 `controller:<run_id>` 主题的 `native_candidates` 为空 —— 控制器自身用量依然无法从 run 账本里被发现（见 §0.4 残留缺口） |
| P17 | 🟢 已覆盖 | `task-workflow.md` + author/verify/rework 三模板要求写明 test owner；F09 |
| P18 | 🟢 已覆盖 | `verify.json` 夹具钉版 + 「在全新可写副本里复现，绝不在存储快照里」；F05/F09 |
| P19 | 🟢 已覆盖 | 上游没有往 `PURPOSES` 里加 `verify`/`rework`（那三项仍是原样），而是新增**与 purpose 正交的 `role`/`phase` 维度**（`usage.py` 的 `DIMENSIONS = ("role", "phase")`），`summary` 现在输出按 `(purpose, role, phase)` 分组的 `groups`。**本机实测**：把 W5 的 V1 样本更正为 `role=verifier / phase=verification` 后，`groups` 正确分组；旧样本缺字段显示 unknown，不推断角色 |
| P20 | 🟢 已覆盖 | `task-workflow.md` rework 条款「ignored-test owner/closure」；F09 |
| P21 | 🟢 已覆盖 | F05 交付快照 + `completion.md:28` handoff 必须绑定同一 job |
| P22 | 🟢 已覆盖 | `task-workflow.md:20` 改为「按项目需要显式声明 fmt/lint/MSRV」；F09 |
| P23 | 🟢 已覆盖 | 上游 A 批已落地：新增 `references/delivery-configurations.md`（交付目标/配置/门禁/制品/覆盖五个必填槽 + 「构建成功 ≠ 行为检查」）与 `assets/verification/rust-cli-plan.json`；`task-workflow.md:20-22` 把 `acceptance-plan.json` 定为「交付目标与必需配置、逐配置 argv/timeout、制品检查、排除项与 owner」。**W5 首次实跑取证**：任务包按 dev / release / release-artifact 分列，作者与验证者都被要求逐配置给结论；控制方在独立构建目录重建 release 制品并比对哈希 → 与作者产物完全一致 |
| P24 | ⬜ 不适用 | 产品缺陷（mini-git 自身代码），按用户裁决保持不修；流程教训见 P23 |
| P25 | 🟢 已覆盖 | 上游 `b124b87` 已补齐三处：`protocol.py` 契约串补「保留原件、正常输出声明更正、等控制器新轮次/path」；`assets/task-packets/author.json` / `rework.json` 各加同一条约束；agent-controlled `protocol.md` 补受控说明（**明确「一个字符的哈希笔误也不允许替换已发布响应」**）。**本机实测**：`grep` 到契约句与两个模板句；运行时 vault 已同步（`cc1a069`）。配套回归在上游 313 项套件里 |
| P26 | 🔴 未处理 | `recovery_delta.py:173-186` 的 `save_cursor` 只在 `state["cache_sha256"]` 变化时写 `contents.json`，但 reset 分支（`:243-249`）把 `state` 换成不含该键的新字典 → reset 路径无条件重写整份缓存。**本机实测** `token_mismatch` / `token_required` 两次：sha256 未变而 `cache_write_bytes` 仍为 203,595（对照组 `0`）。详见 §0.5 / P26 |
| P27 | 🔴 未处理 | `wait_output.py` 有 `offset is not a line boundary` 守卫（实测 exit 2），但 `supervision.md` 未写 `--after` 必须来自上一次返回的偏移 |
| P28 | 🔴 未处理 | `protocol.py write-result` 拒绝重发时抛裸 `OSError`（泄漏 `.result.json.<rand>` 临时名、`kind:invalid`），未说明「已存在有效响应，不要重发」 |
| P29 | 🔴 未处理 | `PUBLICATION_ARGV` 只出现在 `prompt.txt`；`prepare` 的 JSON 响应不含该字段 |

## 0.2 核查基线与复跑方法

| 项 | 值 |
|---|---|
| 项目期间的 skill 基线 | vault `711ed27`（2026-09-19 18:44）|
| 核查时的 skill 基线 | vault `fa1b8d5`（2026-09-20 05:58）；开发仓库 `~/Projects/agent-orchestrator` `0aa3f4a`（06:16）|
| 运行时一致性 | `diff -rq` 比对 vault `agents/<name>` 与仓库 `skills/<name>`：**6 个 skill 零差异**（运行时 = 开发处）|
| 净变化 | `git diff --stat 711ed27 fa1b8d5 -- agents/` → 27 files changed, **+3461 / −115** |
| 上游跟进路线 | `~/Projects/agent-orchestrator/docs/2026-09-19-mini-git-follow-up-plan.md`（F01–F11）；当前状态入口是 `docs/2026-09-20-comprehensive-review.md` |
| 上游自述的空白 | F07/F08（计量）、F10（真实断点覆盖，如长时审批、四宿主组合）、F11（token/wall 对比实验）仍待推进；本轮 268 项回归是 Python unittest + 确定性 fixture，**无真实模型** |

复跑 P1/P2 的最小步骤（2026-09-20 实测有效）：

```bash
cd /tmp && rm -rf p1check && mkdir -p p1check/rounds && cd p1check
T=~/.agents/skills/agent-orchestrator/scripts/protocol.py
# 注意：scope 是字符串（不是数组）；首轮必须显式给 --cwd 和 --parent-depth
printf '{"objective":"P1 check","scope":"probe only","acceptance":["returns a valid request"]}' > packet.json
python3 "$T" prepare --cwd /tmp/p1check --parent-depth 0 --task-packet packet.json --root /tmp/p1check/rounds
# 取输出里的 request_path 后，用当时 100% 报错的字母型 ID 复现：
python3 "$T" record --request <round>/request.json --mode insider --session default \
  --agent x --pane wN:p1 --workspace wN --tab wN:t1 \
  --parent-pane wJ:p1 --parent-tab wJ:t1 --owns-agent --owns-pane --owns-workspace
python3 "$T" cleanup-plan --request <round>/request.json
```

`prepare --previous` 与首轮的参数是**互斥**的：上一轮有有效 result 后，follow-up 要省略 `--cwd` / `--parent-depth`（否则报 `a follow-up inherits cwd and depth; do not override them`）。

---

## 0.3 W5 复验轮发现的问题：上游处理结果（2026-09-20 18:12 之后）

W5 复验轮（用更新后的 skill 跑 `/tmp/w5-linestat`）新发现 4 条，整理成清单
`.orch/upstream/2026-09-20-w5-upstream-issues.md` 交给上游；上游读了该清单并逐条处理，
记录见上游 `docs/2026-09-20-w5-follow-up.md`。

| 基线 | 值 |
|---|---|
| 上游改前 HEAD | `a0fd46a` |
| 上游本批提交 | `b124b87`（2026-09-20 18:12，17 files changed, +358/−10） |
| 运行包同步 | vault `cc1a069`（来源 `b124b87`）；`diff -rq` 6 个 skill 零差异 |
| 上游自报回归 | 313/313 通过（含 13 项真实 tmux）；分发包检查 109 个本地链接零问题 |

| 我们的编号 | 对应 P | 上游处理 | 我的本地实测（在更新后的运行时上跑） |
|---|---|---|---|
| U1 / W5-S3 | 新（未编号） | `capture` 返回与封存 manifest 增加稳定 code 的 `warnings`（`no_baseline` / `no_result`）；交付流程明确 `--baseline` 必填；保留初始快照与旧版读取 | ✅ 不带 baseline → `warnings:[{code:no_baseline}]`，返回与 manifest 一致；带 baseline → `[]` 且 `actual_diff`/`declaration_comparison` 均非 null；W5 旧快照仍 `check` 通过 |
| U2 / W5-S1 | **P25** | 契约串 + author/rework 模板 + agent-controlled 受控说明三处补齐「保留原件、正常输出声明、等控制器新轮次」 | ✅ 三处文案各自 `grep` 命中；受控说明明确「一个字符的哈希笔误也不允许替换已发布响应」 |
| U3 | 新（未编号） | `completion.md` 新增 §"Accepting a verification round"：可执行示例 + **必须同时测已知正确与故意错误** + 拒绝借用作者 attempt | ✅ 按该配方对 W5 的 V1 快照补跑：故意答错的替身 **36/36 全被拒**，真制品 36/36 通过，`passed:true` |
| U4 | **P19** | **未做**，明确归入 D 阶段（role/phase 与 purpose 分离） | ⛔ 仍成立：W5 两个轮次仍只能记成 `task` |
| U5 / W5-S2 | 新（未编号） | `delivery.md` 改成四行类别对照表 + 「四个列表都要读」 | ✅ 文档已就位（代码分类本来就正确） |

**这一批顺带暴露的、我们这侧的缺口**：上游新增的负对照要求让我发现，W5 收口时我验收 V1
**只跑了正例** —— 若 V1 的 harness 是假绿，两边的 36/36 都没有意义（P18 的老问题）。
已补独立 attempt 作为补充证据（`.orch/waves/W5/evidence/v1-acceptance-negative-control.md`），
原记录按「已发布记录不追改」保持原样。

---

## 0.4 上游用量适配批 + 项目改名（2026-09-20 18:33 起）

`b124b87` 之后上游又推了 8 个提交动了 `skills/`，其中两个直接对应本文的计量类发现。

| 基线 | 值 |
|---|---|
| 项目改名 | `~/Projects/agent-orchestrator` → **`~/Projects/kumi`**（旧路径只剩 `.keep-cwd-placeholder`）；另立 `~/Projects/kumi-public` 做公开发布（MIT、`install.sh`、`USAGE.md`、`PUBLISHING.md`） |
| 运行包 | vault `6583f89 docs(agents): follow the source repository rename to kumi`；6 个 skill 与 kumi HEAD `diff -rq` **零差异**，`~/.agents/skills/*` 软链仍有效 |
| 关键提交 | `a58905d feat(usage): adapt OpenCode and Hermes evidence with explicit attribution`、`308b9ca feat(usage): append atomic corrections and import settled turns from live sessions`、`7835b09 Add evidence-backed run and native usage source audits` |
| 上游自报回归 | 331/331 通过（含 13 项真实 tmux） |

| 对应 P | 上游处理 | 我的本地实测（在更新后的运行时上跑） |
|---|---|---|
| **P15** | `native_usage.py` 扩到四宿主：OpenCode 走新增 `native_stores.py`（SQLite/export，assistant message ID 去重、parent user message 映射轮次、`input = input + cache.read + cache.write`、`output = output + reasoning`）；Hermes 走 `hermes_hooks`（累计快照，**明确拒绝**当作 round delta） | ✅ `inspect-native --host opencode --session-id ses_0004…` 解析出 **24 个 assistant call**，input 1,510,971 / cached 1,440,896 / **output 34,572** |
| **P19** | 没动 `PURPOSES`，改为新增**正交的 `role`/`phase` 维度**（`DIMENSIONS`），`summary` 输出 `(purpose, role, phase)` 分组的 `groups` | ✅ 把 W5 的 V1 样本记为 `role=verifier / phase=verification` 后，`summary.groups` 正确分组；旧样本缺字段显示 unknown，不推断角色 |
| **P16** | 新增 `discover-sources`（列 run 内 controller/各 round 的原生候选，只发现不绑定）、`audit-sources`（审计绑定与缺口）、`project-summary`（跨 job 去重） | ✅ 对 W5 的 run 跑 `discover-sources`：两个 round 的原生候选与 ledger 来源都对上了 |

**这一批直接纠正了我们 W5 账本里的一个真实错误。** W5 期间还没有 OpenCode 适配器，V1 的样本是我手工换算的：
当时把原生 `tokens_output`（16,776）直接当成 output，**漏加了 reasoning 细分（17,796）**。
现在用 `usage.py correct` 追加了一条带证据的原子更正（原样本文件不动，effective 版本生效）：

| 项 | 原值 | 更正后 |
|---|---|---|
| V1 output_tokens | 16,776 | **34,572** |
| role / phase | （无） | verifier / verification |
| input / cached | 1,510,971 / 1,440,896 | 不变（说明只错在 output 一侧） |

证据：`.orch/waves/W5/evidence/v1-opencode-native-usage.json`（sha256 `2ca30115…930723`）；
更正记录在 `…/rounds/agent-orchestrator-hgvhj030/usage/corrections/`。
复核：同一条记录用 `inspect-native --host omp` 重算 T1 的样本 → input 1,487,387 / cached 1,451,008 / output 32,135，
与 W5 记的**完全一致**，所以只有 OpenCode 那条需要更正。

**残留缺口**：`discover-sources` 里 `controller:<run_id>` 主题的 `native_candidates` 为空 ——
控制器（codex）自己的用量仍然无法从 run 账本被自动发现或归属，W5 的控制器数字依旧只能从 rollout jsonl 手工切窗得出。

---

## 0.5 上游 C1/C2/B2 批（kumi-public `d90cc8d`）+ 第五次核查（2026-09-21）

上一轮留下的疑问是「vault 比 dev 仓库新」，本轮先把它查清 —— 结论是**仓库拓扑变了，运行时并不陈旧**：

| 仓库 | 状态 |
|---|---|
| `~/Projects/kumi`（私有检出） | **已归档**：HEAD `0ea1a17 docs: archive private checkout and point development to public repo`，不再接受改动 |
| `~/Projects/kumi-public` | **新的权威源**，HEAD `d90cc8d` |
| `~/Projects/agent-skills-vault` | 运行包。`0d8e284 sync(agents): pull the delta-recovery batch and retarget the source at kumi-public` 显式记「origin/main = d90cc8d」，随后 `6852f56` 只动 `PROVENANCE.md`/`README.md` |
| `~/.agents/skills/*` | 仍是指向 vault 的软链，未被旁路 |

**本机复核**：`diff -rq ~/Projects/kumi-public/skills ~/Projects/agent-skills-vault/agents` 只差一个 vault 自有的 `PROVENANCE.md`（六个 skill 目录逐字节相同）。
**结论：运行时就是 kumi-public 的最新内容，无需再同步。**

| 基线 | 值 |
|---|---|
| 权威源 | kumi-public `d90cc8d Improve B2 checkpoint waiting, reporting and supervision` |
| 本批提交 | C1 `7525058` 分页恢复摘要／C2 `ea5e54c` 增量恢复与持久游标／B1 `22f55d6` 恢复元数据误报修复／B2 `d90cc8d` 检查点等待、stdin 发布、监督 |
| 新增文件 | `agent-orchestrator/references/recovery-delta.md`、`scripts/recovery_delta.py`、`scripts/wait_output.py` |
| 上游自报回归 | 492/492（含 13 项真实 tmux） |
| mini-git 侧 | `scripts/check-freeze.sh` → `checked 21 file(s), drift 0`（`src/`、`tests/` 自 09-19 23:18 起全程零改动） |

### 实测（全部在运行时包上、对真实数据跑）

| 项 | 命令 | 结果 |
|---|---|---|
| C1 分页摘要 | `runs.py recover --run <W5>/run.json --summary` | 0.079s；`counts={jobs:2,rounds:2,watches:2,errors:0}`、`complete:true`、`health.success_stale:2`、`mode:summary` |
| C2 首次 delta | 同上 `--delta --limit 10` | `reset:true/initial`，2 个 job + 2 个 watch `upsert`；`read_stats`：71 次源读 / 140,906 B，**同一次调用内 67 次缓存命中**，游标写 854 B、内容缓存写 203,595 B |
| C2 续读 | 带 `--cursor` + `--since <上次 next_cursor>` | `reset:false`，`change_counts={jobs:0,watches:0}`；源读 71→**1**（495 B），`cache_read_bytes:203,595`、`cache_write_bytes:0` |
| C2 丢失输出 | 带一个不匹配的 `--since` | `reset:true / token_mismatch`，全量重放 2+2 条 —— 不静默漏行，与文档一致 |
| C2 非法游标 | 指向 run 外的路径 / 指向软链 | 均 exit 3：`cursor must be this run's recovery/<id>/cursor.json`（既不写入也不覆盖） |
| C2 损坏游标 | 把 `cursor.json` 写成非 JSON | `reset:true / cursor_invalid`，`cache_read_bytes:0`（缓存整份丢弃后重建） |
| **C2 零副作用** | 对 `orch-run-2lpx81iy` 拍 429 个文件的 sha256 清单 → 跑 6 次 delta → 比对 | **0 个既有文件被改、0 个被删，只新增 `recovery/<id>/{.lock,cursor.json,contents.json}` 3 个文件**；事后删除 `recovery/`，清单与测试前**逐字节相同**。文档「Existing task/results/reviews are never modified by delta」成立 |
| B2 检查点等待 | 真实 producer（3×0.3s 后写 `CHECKPOINT_READY`，再睡 30s）+ `wait_output.py --timeout 15` | 从「文件还不存在」开始等，**0.40s 命中并 exit 0，此时 producer 仍在运行** —— 证明了「原生 wait 打在 helper 上可以提前返回」这个核心诉求；`after:25`、`identity:"39:5675095"` 可续用 |
| B2 续读 / 超时 | `--after 25` 等 `STREAM_FINISHED` | 立即命中（`after:49`）；无新行时 `status:timeout` exit 1 且保留可续游标 |
| B2 守卫 | 部分行 / 符号链接 / FIFO / >64KiB 行 / 同 inode 被截断 / 源被替换 | 全部按文档拒绝：未换行的部分行不算命中；软链 `ELOOP`、FIFO `expected regular log file`、超长行 `log line exceeds size limit`、截断 `log truncated or offset beyond EOF`、替换 `log identity changed`（均 exit 2）；**全过程没有生成任何输出文件** |
| B2 发布配方 | `prepare` → 取 prompt 里的 `PUBLICATION_ARGV` → 无 shell 执行 + stdin 关闭 | exit 0，`result.json` 落位；轮次目录里**只有** `request.json/prompt.txt/run-ref.json/result.json`，**零候选文件**；随后 `validate --check-files` exit 0（故意少建一个声明文件时被正确拒绝） |
| **W5-S1 回归** | 对已发布的轮次再发一次（只改 output / 改成 blocked / 改 job_id） | 原 `result.json` **sha256 不变**；重发一律被拒；改 `job_id` 报清晰错误 `response job_id does not match this round` |

**一个要写下来免得下次误判的行为**：run 目录**不能搬迁或改写**。把 W5 的 run 复制到 `/tmp` 后，
`recover` 直接报 `run path/temporary flag changed`；只改路径字段不重算哈希，又会退化成
`index identity/revision mismatch` / `request run membership changed`（10 条错误）。
这是**完整性设计**（索引与 request 都钉了路径归属和 sha256），不是缺陷 —— 但也意味着
「离线复制一份 run 再分析」这条路走不通，只能在原地读，或走 delta 这种只写 `recovery/` 的方式。

### 这一批新发现（P26–P29）

| # | 严重度 | 一句话 |
|---|---|---|
| P26 | 🟡 成本向 | 只要这次调用触发 reset（`initial` / `token_mismatch` / `token_required` / `version_changed`），就会**无条件重写整份 `contents.json`**，即使内容逐字节没变 —— 与 `recovery-delta.md`「The content file is rewritten only when its payload changes」不符 |
| P27 | 🟢 文档 | `--after` 必须落在行边界，否则 exit 2 `offset is not a line boundary`；该前提没写进 `recovery-delta.md` / `supervision.md` |
| P28 | 🟢 可用性 | 重发已发布 result 的拒绝理由是**裸 OSError** 且泄漏临时文件名（`File exists: '…/.result.json.5n56bc02' -> '…/result.json'`），`kind:invalid`。对 W5-S1 那个场景的 agent 不可读 |
| P29 | 🟢 文档 | `PUBLICATION_ARGV` 只出现在 `prompt.txt` 里；`protocol.py prepare` 的 **JSON 响应里没有**，自动化 controller 必须去读 prompt 文件才能拿到 |

---

## 0.6 W7（收口轮）实测与第六次核查（2026-09-21）

W7 是本项目的**收口轮**：唯一目的就是修掉 P24 并把它验完。它是**第一次**让作者与验证者在
「交付物 + 封存快照」上真正对接的一轮，因此也第一次把下面这几类问题照了出来。

### 这一轮新发现（P30–P34）

| # | 严重度 | 一句话 | 抓到它的是 |
|---|---|---|---|
| P30 | 🟠 严重（产品向） | 测试套件用 `CARGO_MANIFEST_DIR` + **append** 把证据写回仓库 → 每跑一次 `cargo test` 就往里追加一份，历史证据被污染，且**让这个仓库无法通过 `kind=delivery` 验收** | 控制方对作者交付做 `delivery.py verify` 的 `source_unchanged` 检查 |
| P31 | 🟡 中等（可信度向） | `[profile.release] debug = 1` 让**构建路径内嵌进制品**：验证者重建的 sha256 必然与作者不同（本轮 `21b5679f…` vs `c3e51adc…`），差异 40 字节 = 4 处路径字符串 —— P21 那套「冻结制品 sha256」的配方在跨目录场景下会**假红** | 验证者自证（.text 段逐字节相同 + `--remap-path-prefix` 重建复现作者哈希）+ 控制方复核 |
| P32 | 🟢 轻微（流程文档向） | 「验证者轮要在开工前抓 `V_BASELINE`」对**验证者自建工作目录**（`/tmp/w7-verify` 这类）根本做不到 —— 目录在轮次开始前不存在，capture 必然带 `no_baseline`，`actual_diff`/`declaration_comparison` 为 null。文档说了「不能补」，但没给「这类轮次该抓哪个 scope」的配方 | 控制方做 V2 的 `delivery.py capture` |
| P33 | 🟢 轻微（可用性/安全向） | **lease token 不是秘密**：`claim` 的回复把整个 state（含 `lease.token`）返回，且同一份 state 原样落盘在 `<index>/<job>/<rev>.json`。`recover` 有意不带 token，但**任何能读索引目录的进程都能直接读出 token 接管所有权** —— 与「never infer ownership」「过期 token 不能改索引」的措辞相比，实际保证弱得多 | 控制方丢失 claim 回复后，直接从 history 文件取回 token，省掉 30 分钟等待 |
| P34 | 🟢 轻微（可信度向） | 验证者脚手架**判定失败也 exit 0**（`scenario/run_scenarios.py` 只把 failures 写进 stdout/json）。`delivery.py verify` 的每条命令要求 `exit_code == 0` —— 只看退出码的控制方会把红判成绿 | 控制方读验证者的脚本 + 写验收计划时的显式断言 |

### 这一轮的正面证据（同样要记）

* **交叉验证的价值再次被证实**：P24 是四个 wave、变异测试、字节级对拍都没抓到的**profile 盲区**，
  而这一轮「不同 kind 的验证者 + 自己写的 oracle + 必做负对照」把它钉死了。
* **「作者主张」与「验证者证据」被真正分开**：V2 只把作者的 `result.json` 当 claim，
  制品 sha、语料、超时策略全部自建；控制方再独立重建两份制品复跑同一脚手架。
* **协议对「交付即冻结」是够用的**：`delivery.py capture` 把 job/round/request-sha/result-sha
  与 384 个文件逐个哈希绑在一起，`verify` 之后 `source_unchanged` 仍为 true，
  作者/验证者的后续写入都落在快照之外。
* **`begin` 与 `send` 分离救回了一次调度错误**：V1 被排在作者交付之前（W7-S1），
  取消发生在 prompt 送出之前 → 只损失控制方时间，没有浪费 agent token。

---

## P1（🔴 阻塞）insider 模式下 record / watch / cleanup-plan 全部不可用

> **上游现状（2026-09-20 核查）：✅ 已修。** `protocol.py` 现用 `HERDR_PUBLIC_NUMBER = r"[123456789ABCDEFGHJKMNPQRSTVWXYZ0]+"`（herdr v0.9.1 的公开编号字母表），并写明「ID 是不透明标识：只校验形状，不解码、不重编号」——比本文建议的 `[A-Za-z0-9]+` 更保守。**本机实测**：`record` 接受 `--pane wN:p1 --workspace wN --tab wN:t1`（当时 100% 报错），`cleanup-plan` 正常输出计划，非法 ID（`w1:p`）仍被拒。上游对应 F01（`999296a` → vault sync `135d44a`）。

**现象**：在用户既存会话（`herdr --session default`）里用 `workspace create --no-focus`
建可见子 agent 后：

```json
{"error": "herdr requires an exact pane ID such as w1:p2, not a current/focused alias", "kind": "invalid"}
```

同一条命令在**新建**会话（isolated）里却完全正常。

**证据**：
- herdr 原样返回的精确 ID 是字母型：`{"root_pane":{"pane_id":"wN:p1","tab_id":"wN:t1","workspace_id":"wN"}}`；
- `agent start ... --pane wN:p1` 成功（`interactive_ready: true`）→ 证明 ID 有效；
- `record` / `watch.py init` / `cleanup-plan` 三个命令都报上面那条错。

**根因**：`protocol.py` 的 `validate_resources()`（`record`、`cleanup-plan`、`prepare --previous`、
`watch.py init` 都走它）用

```python
re.fullmatch(r"w[0-9]+:p[0-9]+", pane)          # 以及 w[0-9]+ / w[0-9]+:t[0-9]+
```

校验 pane/workspace/tab。该正则只覆盖**新建会话**的 ID 形态（`w1:p1`、`w2:p1`…），
而 herdr 对既存会话按 base-26 风格递增分配（`wN`、`wP`、`wQ`…）。
**结果：最常用的"在用户自己的会话里开可见 space"这条路径，工具链是断的。**

**最小复现**：

```bash
herdr --session default workspace create --cwd /home/user/Projects/mini-git --no-focus
# 读回 .result.root_pane.pane_id = wN:p1

python3 ~/.agents/skills/agent-orchestrator/scripts/protocol.py record \
  --request <round>/request.json --mode insider --session default \
  --agent x --pane wN:p1 --workspace wN --tab wN:t1 \
  --parent-pane wJ:p1 --parent-tab wJ:t1 --owns-agent --owns-pane --owns-workspace
# → invalid: herdr requires an exact pane ID such as w1:p2

python3 ~/.agents/skills/agent-orchestrator/scripts/protocol.py cleanup-plan --request <round>/request.json
# → 同一条错误（第三次复现）
```

**影响**：
1. `resources.json` 只能手工按同 schema 落盘（本轮已这么做，附 `record_note` 说明）；
2. `cleanup-plan` 不可用 → 收尾必须手工核验 + 按各 target 的退出命令处理；
3. `watch.py` 不可用 → 监督退回 skill 文档允许的"manual supervision loop"；
4. 只有 **isolated 模式（新建会话）**能完整走通协议 —— 而那个模式下用户**看不见**子 agent。

**建议修复**（一处，全部解开）：

```python
# protocol.py
- re.fullmatch(r"w[0-9]+:p[0-9]+", pane)
+ re.fullmatch(r"w[A-Za-z0-9]+:p[0-9]+", pane)
- re.fullmatch(r"w[0-9]+", workspace)
+ re.fullmatch(r"w[A-Za-z0-9]+", workspace)
- re.fullmatch(r"w[0-9]+:t[0-9]+", tab)
+ re.fullmatch(r"w[A-Za-z0-9]+:t[0-9]+", tab)
```

（或更好：不猜 ID 语法，只校验「非空 + 无控制字符 + 含 `:` + 与 workspace 前缀自洽」。）
`watch.py` 若复用同一模块则自动受益。

---

## P2（🔴 阻塞流程）follow-up 轮次被同一校验阻断

> **上游现状（2026-09-20 核查）：✅ 已修。** **本机实测**：上一轮写出有效 result 后，`prepare --previous` 成功，且 job_id 保持不变、depth 自动继承为 1、cwd 继承；显式再传 `--cwd` / `--parent-depth` 会被新加的守卫拒绝：「a follow-up inherits cwd and depth; do not override them」（`protocol.py:231-235`）。`validate_resources` 仍会读上一轮的 `resources.json`，但字母型 ID 现在能通过（见 P1）。

`prepare --previous <上一轮 request.json>` 会读取上一轮的 `resources.json` 并再次
`validate_resources` → 在 insider 模式下必然失败。于是**返工/追问轮次**只能改用「全新 prepare」，
代价是丢失 job 关联（需要 controller 在任务包里手工带上上下文）。

**证据**：`protocol.py` 第 208-210 行与第 369 行都调用 `validate_resources`。

---

## P3（🟠 严重）hermes 轮次结束后的自动「self-improvement」= 协议外持久副作用

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** 新增 `agent-hermes/references/isolation.md`（每轮独立 Hermes storage、启动前准备、退出后用同一 home 恢复、retire 前先校验 profile）；`completion.md:47-48` 要求收尾时记录「native child lineage and observed side effects, including relevant paths **outside cwd**」，协议外副作用被纳入完成检查；另加了「首发结果钉存、不可改写」。上游对应 F06 + 发布隔离批次。

**现象**：hermes 交付 result 之后，会立刻触发一次 curator 复盘，**改写它自己的私有 skill**：

- `~/.hermes/skills/contract-driven-module-delivery/SKILL.md`（改写）
- `~/.hermes/skills/contract-driven-module-delivery/references/reference-binary-oracle.md`（新增）
- 备份进 `~/.hermes/.curator_backups/blobs/**`
- 账本：`~/.hermes/skills/.curator_ledger.jsonl`（`actor: curator`，带 before/after sha256）

**为什么严重**：
1. 这是**cwd 之外**的持久写入，而 `result.json` 的 `files_*` 按契约只能是 cwd 内相对路径 →
   **监督方只看 result 文件时完全发现不了**；
2. 它发生在**轮次结束之后**，也就是"任务已完成"的时刻，容易漏检；
3. 它会计费：hermes 每轮固定多 7 次 `background_review` 调用（T2 $0.0104、V4 $0.0129）。

**W2 复现（非偶发）**：V8（hermes 验证者）交付 result 后，面板上再次出现
`💾 Self-improvement review: Skill 'contract-driven-module-delivery' patched ·
Skill 'contract-driven-module-delivery' patched (references/independent-verification.md)`。
即：**同一行为在作者轮（W1/T2、W1/V4）与验证轮（W2/V8）都出现**，与任务类型无关，
是每轮固定的收尾动作；而且它**会新增 reference 文件**（`references/independent-verification.md`），
说明副作用面在持续扩大。

**本轮影响**：可控（动的是 hermes 私有 skill，不是用户共享的 `~/.agents/skills`，
也不是项目文件），但**对任何以写作用域为硬约束的编排都必须算越界**。

**建议**：
1. 任务包显式要求「不要在工作区外产生持久副作用」并让 agent 在 `output` 里披露；
2. 监督方在 hermes 轮次后固定检查 `~/.hermes/skills/.curator_ledger.jsonl` 的
   `evidence.session_id` 是否等于本轮 session；
3. 若不允许，需在 hermes 侧关闭 curator（会改变被测行为，需用户同意）。

---

## P4（🟠 严重）`agent start` 在既存会话里偶发超时，且重跑会重复输入

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** `references/transports.md:205` 把本文的恢复配方写成了正式条款：startup 未注册时用 `agent rename` 绑定再用 `agent get` 复核；「Record this as startup failure followed by recovery; retrying `agent start` could duplicate input」；**只允许一次恢复尝试**，仍不确定就停。

**现象**：`herdr agent start orch-codex-v3 --kind codex --pane wS:p1` 返回

```json
{"error":{"code":"timeout","message":"timed out waiting for agent startup"}}
```

但 `pane process-info` 显示前台进程只有 `/usr/bin/fish`，屏幕上是**已输入未执行**的 `codex`。

**处理（skill 文档里的配方有效）**：

```bash
herdr --session default pane send-text wS:p1 $'\r'     # 只发一次回车
herdr --session default agent rename wS:p1 orch-codex-v3   # 因为 agent start 失败，未注册
herdr --session default agent get orch-codex-v3        # 确认
```

1 次恢复即成功，**没有**重试 `agent start`。

**坑**：这是个**静默的重复启动陷阱** —— 如果不看进程表就重跑 `agent start`，
会在同一个 TUI 里输入第二份启动命令。skill 文档已警告，但只有在真踩到时才知道它值多少。
另注：T1 试运行的 isolated 新建会话**没有**出现该问题。

---

## P5（🟠 严重，成本向）codex 的原生审批是唯一吞吐瓶颈

> **上游现状（2026-09-20 核查）：🟡 部分缓解，根因保留。** 方案明确**不**用 bypass 解决：`agent-codex/SKILL.md` 要求不自动切换模型、不清配置、不启用 `--dangerously-bypass-approvals-and-sandbox`，并写明「不能为了让测试通过而关掉审批」。巡视侧补了纪律：同一个未决问题只中继一次、记成 `pending` 后不重复问、一个目标的审批不得饿死其他目标（`lifecycle.md:46/52`）。**codex 对复合 bash 逐条弹审批这件事本身没变**；上游自己也承认 30 秒响应目标本轮未达标（F10）。

**实测**（W1）：

| agent | 轮次内审批次数 | 单轮耗时 |
|---|---|---|
| codex（T4 作者）| 4 次「1. Yes, proceed / 2. No」| 13m53s（W1 最长）|
| codex（V3 验证者）| 4 次（其中一次是 2 选项版）| 15m58s（W1 最长，另含 P4 恢复）|
| hermes / opencode / omp | **0 次** | 4–5 分钟 |

- 每次弹窗都对应**一条复合 bash 命令**（多行脚本）。
- 全部按任务包纪律选「1 单条批准」，**从未**使用「2. don't ask again」。
- 代价：审批往返期间该 agent 完全停滞，controller 也不得空闲（要读屏判断）。

**建议**：
1. 任务包里把"允许跑的验证命令"写成**可预期的固定清单**，减少 agent 临场拼长脚本；
2. 明确禁止「为了少弹窗而把命令合并成巨大脚本」——那会让审批更难判断；
3. **不要**通过关闭审批来优化（那会改变被测行为）；
4. 若要压成本，正确做法是让验证者用**一个**测试文件 + 一次 `cargo test`（W1 的 V3/V4 都这么做），
   而不是反复跑一次性脚本。

---

## P6（🟡 中等）`watch.py` 对持续重绘 TUI 的噪声

> **上游现状（2026-09-20 核查）：✅ 已修。** `efficiency.md:84`：changed-screen `review_due` 起步延迟 30 秒，之后退避 60/120/240/300 秒，画面变化或观测中断会重置退避，显式疑似对话框每轮照测；`:102`：同一未决问题的重复提醒合并为最新一条，带 `occurrences` 与最早 `detected_at`。上游对应 F04（`a84afff`「cut supervision noise and separate review from resolution」）。

T1 试运行实测：8 次 `review_due` 里 7 次是纯噪声（spinner/进度行重绘触发 changed-screen 启发式）。
结论：对持续重绘目标，`review_due` 应降级为"采样"而不是"必须处理"，
真正要盯的是**状态变化**与**关键词**（`Yes, proceed` / `Ask` / `1.` 菜单）。

---

## P7（🟡 中等）`agent get` 的状态不可信，且跨 kind 不一致

> **上游现状（2026-09-20 核查）：🟡 部分缓解。** `agent get` 仍用于就绪复核，但不再被当作开工/成功的证据：SKILL.md 明写「CLI success alone is not confirmed acceptance」，`jobs.py` 引入 `begin`（发指令前先记为 `uncertain`，一个 attempt 只能有一次）与 `receipt`（须给出**匹配的证据种类**）。**「跨 kind 状态语义不一致」这一层没有被上游定义。**

| kind | 观测 |
|---|---|
| omp | `screen_detection_skipped: true`，工作期间长期报 `idle` |
| codex | 基本准确，但 W1 出现过 `blocked` 持续 3 分钟而 pane 内仍在推进、随后正常出结果（疑似状态机滞后）|
| hermes / opencode | `done` 与结果文件一致，本轮无误报 |

**结论（已验证的做法）**：判定一律以 **`pane read` 的屏幕 + 结果文件**为准，
`agent get` 只当辅助信号。

---

## P8（🟡 中等）轮次结束后仍会弹**产品级**对话框

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** `agent-omp/references/lifecycle.md:12/47`：发布响应后必须看真实画面，产品提问 / permission 对话框 / 新提示分别处置，并「Record product dialogs as waiting and unavailable background/child coverage as unknown」。上游对应 F06 宿主生命周期。

omp 在**发布 result 之后**弹出「要不要花 10-15 分钟做个学习练习」Ask 对话框，
并把 `agent status` 置为 `blocked`。

关键点：**任务包里已经明写"不启动学习练习/引导流程、不弹 Question/Ask"，仍然会弹**
——因为它是产品侧在轮次结束时主动发起的，不看任务包文字。

**处置**：`send-keys down` + `enter` 选「不要」。
**教训**：这类交互**无法靠任务包规避**，只能由监督方常备处理。

---

## P9（🟡 中等）共享检出 + 共享 `target/` 的互相污染

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** 新增 `references/delivery.md` + `scripts/delivery.py`（495 行）：源码快照 + 逐文件哈希/大小/可执行位、交付清单（job/round、request digest、scope、handoff）、**独立 build/cache/evidence 目录**、命令「非零退出 / 超时 / 源码变更即停」。上游对应 F05。

1. **编译噪声**：W1 三个作者共享一个 checkout，任何一个半成品语法错误都会让别人的
   `cargo test` 假红。任务模板已加规则：*白名单外的编译错误不归你，等 30 秒重试，
   连续 3 次失败就记录并报 blocked，不许改别人的文件*。
2. **cargo 缓存被覆盖**：V2（omp）做变异测试时直接改共享 `target/`，
   同名 crate 的产物互相覆盖；它自己识别后改用隔离副本重跑（V3 用 `/tmp` 副本，没踩到）。
   模板已加规则：*cargo 锁等待是正常的，但**变异/临时构建必须用独立 target 目录或独立副本***。
3. **环境变量泄漏**：V4（hermes）在持久 shell 里 `export GIT_AUTHOR_NAME=a` 做实验，
   泄漏进 shell-out 到真实 git 的子进程，导致 **T1 的验证用例假红**（`'a' vs 'A U Thor'`）。
   模板已加规则：*不要 export 会被 git 读取的环境变量，用 `git -c ...` 或 `env VAR=... cmd`*。

**注意**：skill 的 transport 文档只说「并行写者要给独立目录/明确不相交的文件所有权」，
**没有提 target 目录、环境变量、变异构建**这三类共享资源。这是文档的空白。

---

## P10（🟢 轻微）任务包规格错误只能靠真值 oracle 暴露

> **上游现状（2026-09-20 核查）：🟡 部分缓解。** `task-workflow.md` 把「任务书与 oracle 冲突 → 返回 `blocked`，同时给出双方期望与最小反例，控制器在新一轮解决，作者/验证者都不得静默改期望」写成正式流程。但**发现**规格错误这件事仍然依赖人或验证者用真值 oracle 去打。

- 我把 `git status --porcelain` 的排序写成"行按 path 字节序"，
  正确规则是 **先 changed 行、再 `??` 行，组内才排序**。
  W1 里**两个独立 agent**（作者 codex、验证者 hermes）各自用真实 git 对拍发现并纠正。
- 另有一处规格二义：`resolve("feature/x")`（含 `/` 的短名）该不该回退到 `refs/heads/<name>`，
  真实 git 会回退，任务包文字有二义 → 被验证者标为「未覆盖」而非 FAIL（处置见决策记录 C-12）。

**W2 新增两例**：
- `T5-odb/task.md` §3.2 说 tree 的 mode 显示成 5 位 `40000`，但 §4.6 又要求
  `mg cat-file -p <tree>` 与 `git ls-tree` 逐字节一致；实测 git 2.55.0 两者都打印 `040000`，
  5 位只出现在 tree **载荷**里。作者 hermes 用真实 git 对拍后按 git 处置，并**主动上报冲突**。
- `T8-pktline/verify-task.md` 把 `git ls-remote <path>` 的 stdout 当成「原始 pkt-line 字节流」，
  实际它是纯文本 `oid\tref` 行。验证者 hermes（V8）实测后改为
  `git upload-pack --stateless-rpc --advertise-refs`，并在报告里**更正任务书**（见 P11）。

**做法（已采用）**：任务包**原样保留**（保留"spec 有错"的证据），
修正写进 `ORCHESTRATION.md` 的决策记录与 wave 复盘。
**教训**：真值必须来自被测对象本身（真实 git），否则规格错误会被固化进实现。

---

---

## P11（🟠 严重，规格向）任务书指定的「真值来源」本身可能是错的

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** `verify.json` 要求钉死 oracle 身份/版本/夹具哈希，并要求「先用一个已知错误的输出试一次 oracle，必须被拒绝」；`task-workflow.md` 收尾句：「Self-consistent fixture expectations alone do not establish that the verifier rejects wrong result shapes」。这正是 W2/T8 的教训。

**现象**：验证任务书由 controller 手写，其中的"真值来源"只是 controller 的**记忆**，
一旦记错，验证者如果照抄就会写出**看起来在独立验证、实际在验证错误真值**的测试。

**实测两例（都在 W2）**：
1. `T8-pktline/verify-task.md`：「`git ls-remote <path>` 的 stdout（含 flush-pkt 结尾）」——
   错。V8 实测该输出是纯文本 `oid\tref` 行，没有长度前缀；正确来源是
   `git upload-pack --stateless-rpc --advertise-refs <gitdir>`（另外还补了
   `git http-backend` + `GIT_PROTOCOL=version=2` 作为第二数据源）。
2. `T5-odb/verify-task.md` 与 `task.md` 对 tree mode 位数的要求互相矛盾（见 P10）。

**为什么算"严重"**：这类错误**不会自己暴露**——如果 agent 忠实地按错误来源实现断言，
它会得到"自洽的假绿"（用 `encode_pkt` 造 fixture 再断言 `read_pkt` 能读，就是这种失败模式的极端）。
本次能发现，是因为**验证者主动质疑了任务书指定的 oracle**（V8 报告里直接写「更正任务书」）。

**建议**：任务书只写「真值意图」（例如"必须与真实 git 产出的字节流逐帧逐字节一致"），
**不要写死具体命令**；或明确写「命令只是提示，你需要自己确认它真的是该真值的来源」。

---

## P12（🟡 中等）`prepare --parent-depth` 的语义与任务书里写死的 depth 数字不一致

> **上游现状（2026-09-20 核查）：🟡 部分缓解。** 首次 `prepare` 仍必须显式给 `--parent-depth`；但 follow-up 已自动继承且**禁止**覆盖（见 P2），错误信息也从裸校验变成行动指引（`protocol.py:194-195`「Depth counts the root as 0… at depth=max_depth-1 do the task locally and do not delegate」、`:248`「depth limit reached: execute locally or report inability to delegate」）。上游 F09 自述「depth 由 prepare 生成，不在业务任务中手填矛盾数字」。

**现象**：controller 想产出「depth: 1」的轮次（与 W2 已发的 V8 轮次一致），
于是按直觉传 `--parent-depth 1 --max-depth 3`，结果 `request.json` 里是 **`depth: 2`**。
也就是说 `--parent-depth N` 是「父轮次的 depth 是 N」，子轮次 = N+1，而不是「本轮的 depth」。
正确写法是 `--parent-depth 0`。

**影响**：协议层面不致命，但
1. `prompt.txt` 的身份块会写 `depth: 2`，而任务书正文又写「本轮的 depth：1」，
   同一份材料里出现两个不同的 depth，agent 可能据此误判自己的委派权限；
2. controller 必须**试错一次**（我第一次就错了），并需要删掉误建的 round 目录。

**证据**：`prepare --parent-depth 1` → `agent-orchestrator-nl60jxdq` 的 `depth: 2`；
改 `--parent-depth 0` → `agent-orchestrator-i2muklue` 的 `depth: 1`。
**建议**：参数改名为 `--parent-depth` 已算清楚，但文档/帮助文本没说明「子轮 = 父轮 + 1」；
或直接提供 `--depth`。

---

## P13（🟡 中等）并发 wave 里「谁改了哪个文件」协议给不出答案

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** 归因从「mtime 时间窗」升级为「交付时点快照 + 哈希」：`delivery.py` 的 manifest 记 scope、handoff、逐条哈希与 request digest，并要求多模块集成在明确的集成版本上验收。注意其自述边界：`delivery.md:145`「证据哈希能检测变化，不是防同用户篡改的签名」。

**现象**：W2 是 4 个作者 + 4 个验证者**同时**在同一个 checkout 里工作。
`result.json` 的 `files_modified` 是**作者自报**；`drift.sh <baseline>` 只能告诉你
「当前工作树 vs 开工基线」的差异，**不会区分是哪个 job 改的**。
于是验证者 V8 只能这样归因：拿基线清单 diff 出 10 个变动文件，
再用 **mtime 窗口**（作者 result 的写入时间）判断哪个属于自己、哪个属于并发的 T5/T6/T7。

**为什么要记**：这是「越界判定」这类核心结论的**唯一证据来源**，但它依赖
「mtime 落在窗口内」这种**可被伪造**的启发式（agent 完全可以改写 mtime）。
W2 里还出现了瞬时编译失败（`src/diff/myers.rs`、`src/cli/cat_file.rs` 正在被写）
被验证者正确标注为「不可归因于本任务」——处理得当，但整套机制靠的是人/agent 的自觉。

**建议**：协议补一个 controller 侧的「快照」动作：每轮开始时记录
`sha256 + mtime + inode` 清单，轮次结束时再记一次，用**集合差 + 窗口**双重确认；
或让 agent 在完成时输出自己所有写操作的 sha256。

---

## P14（🟢 轻微）`agent prompt` 返回的状态不代表"已开工"

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** `jobs.py` 的 `begin`/`receipt` 语义 + SKILL.md「`prepared_at` 和 CLI 成功都不是已接受的确认」+ `efficiency.md:78`「state is not round success」+ `runs.md:141`「不是原生对话框起点、控制器唤醒时刻或审批响应延迟」。

**现象**：`herdr agent prompt <name> "<text>"` 的返回值里带 `agent_status`，
但那是**投递前**的旧状态。W2 实测 5/5 次都返回 `idle`（连 `revision` 都没变），
随后 agent 实际开始工作。

**影响**：若用返回值判断"是否已经开始干活"会误判为投递失败。
**做法**：以 `revision` 增长 + 结果文件出现 + `pane read` 三重判定（W1 已用，W2 继续有效）。

---

## P15（🟡 中等）原生 token 计数只覆盖 codex/omp，跨 kind 成本对比不成立

> **上游现状（2026-09-20 核查）：🔴 上游仍未处理。** `scripts/native_usage.py:57` 仍是 `host in ("codex", "omp")`，`usage.py` / `native_usage.py` 的最后修改时间仍是 09-19 18:37（本轮其余脚本都改过）。上游 F07 明确列为待办，并采纳了本文的结论原话：「Hermes/OpenCode **缺适配器**，不是缺数据」。

**现象**：`scripts/usage.py inspect-native --host` 只有 `codex` 与 `omp` 两个选项
（`usage.py:169`）。hermes / opencode 的轮次**没有原生读数**；面板上显示的
（`71.3K/1M`、`139.8K（14%）`）是**当前上下文占用**，而原生导入得到的是**累计调用量**
（例如 T6 一轮 90 次调用、累计 input 16.1M、其中 cached 16.0M）。两者量纲不同，
W1 的账本把两种数字并列，W2 起已分开记。

**证据（W2 实测，可复核）**：
```bash
python3 ~/.agents/skills/agent-orchestrator/scripts/usage.py inspect-native --host omp \
  --log ~/.omp/agent/sessions/-Projects-mini-git/2026-09-19T12-13-15-961Z_sess_0010-*.jsonl
# → {"session_id": "sess_0010-...", "turns": [{"native_id": "55c495dc", "calls": 90,
#     "known_subtotals": {"input_tokens": 16138242, "cached_input_tokens": 16034048, "output_tokens": 169972}}]}
```
`usage.py summary --request <round>/request.json` 能把导入的样本汇总成
`known_subtotals` / `totals` / `rounds_without_usage`，**这一点很好用**（W2 的 6 个 codex/omp 轮次
全部导入成功、`unmapped_calls=0`、重复导入幂等）。

**建议**：要么给 hermes/opencode 补适配器（它们各自的 session 日志里很可能有 usage 字段），
要么在 skill 文档里明确「跨 kind 成本不可比，只能同 kind 比较」。

**✅ W4 收口的实测补充：这两个 host 的用量其实都拿得到**（无需改 skill，只是没适配器）：

| host | 数据来源 | 本轮核出的用量 |
|---|---|---|
| hermes | `~/.hermes/state.db` → 表 `session_model_usage`（`api_call_count` / `input_tokens` / `cache_read_tokens` / `output_tokens`，按 session×model 分行）| 7 会话 / 544 calls / 70,834,498 input（70,037,120 cached）/ 608,216 output |
| opencode | `~/.local/share/opencode/opencode.db` → 表 `message.data`（JSON 里的 `tokens.{input,output,cache.read,cache.write}`），用 `session.directory` 过滤 | 10 会话 / 527 calls / 72,642,866 input（71,655,680 cached）/ 247,338 output |

两者都是 **SQLite**，都带 `session_id`/时间戳/目录，**完全可以写适配器**（比 codex 的 JSONL 更好接）。
因此 P15 的严重度可以降级为「适配器缺失」，而不是「数据缺失」——
跨 kind 成本对比在数据层是**可做**的，只是 `usage.py` 没做。

**顺带发现**：hermes 侧的 `sessions.source` 出现了一个 `subagent` 会话（2026-09-19 21:07:45，
29 calls / 0.29M input），说明 hermes 在 W3/V10 那轮**自发做了嵌套委派**。见 §2 的修正。

---

## P16（🟢 轻微）原生导入需要逐轮手工建立 native↔round 映射

> **上游现状（2026-09-20 核查）：🔴 上游仍未处理。** 上游 F07 待办写明「manifest 的手工映射成本仍大」，计划方向是「启动/续轮记录原生 session/turn + 只读发现候选 + 显式绑定，不按时间自动认领」。本轮落地的 `runs.py` 持久 run 给了归属一个**容器**，但 native↔round 的映射本身仍需手工建立。

**现象**：`import-native --manifest` 要求 manifest 里写死 `log_path`、`session_id` 与每个
`native_id`（codex 的 turn id / omp 的 user-message entry id），而**控制器只能靠时间戳**
去猜哪个 session 属于哪个 round（pane、workspace、agent 名都不在 native 日志里）。
W2 的 6 个 manifest 是我逐个比对 `ls -l` 时间与轮次时间手写的。

**影响**：轮次一多（W2 有 6 个 codex/omp 轮，W3 预计更多），映射就是纯人工活；
一旦映射错，样本会永久挂在错误的 job 上（重复导入按 `(source, sample_id)` 去重，**不会**纠正归属）。

**建议**：`agent start` 或 `prepare` 时把 native session 落进 `resources.json`；
或在 `import-native` 里支持「按时间窗口 + cwd 自动候选」。**本次 W2 的映射已存 `.orch/artifacts/usage-manifests/`**，
用户可复核。

---

---

## P17（🟠 严重，流程向）门禁测试的「所有权」没有被方案定义

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** `task-workflow.md` 与三个模板把 test owner 写成了必备事实：author 模板要求交付时停止自有 writer 并列出未决依赖；verify 模板要求写明 test owner、且「product verdict 与 protocol status 分开报」；rework 模板要求写明 characterization 替换与 ignored-test 的归属和关闭条件。上游对应 F09。

**现象**：随着被测项目推进，**早先 wave 写的测试会开始断言旧世界**，从而把后来的 wave 门禁弄红：

| 测试 | 写的 wave | 变红原因 | 谁该修 |
|---|---|---|---|
| `tests/interop/smoke.rs::unimplemented_commands_fail_loudly...` | W0（controller-owned） | 它断言「`mg status` 必须报 not implemented」，而 status 在 W3/T12 实现了 | controller |
| `tests/verify_worktree.rs::deviation_type_change_regular_to_symlink` | W1（验证者的文件） | 它把「mg 用 `M` 而 git 用 `T`」记成**已知偏差**，C-13 修好后偏差消失 | controller |
| `tests/verify_materialize.rs::symlink_ancestor_write_stays_inside_worktree` | W3/V11（验证者的文件） | 它断言的是 mg 的**旧保守行为**，而 T11b 要对齐 git | V11b（同一验证者） |

**为什么算「严重」**：方案的写作用域纪律（`blocked` 而不是越界改）是**对的**，
但它把「测试过期」这类**编排侧债务**全部压到 controller 身上；而 controller 在 wave 之间
并没有「谁负责让门禁与当前实现同步」的清单。W3 一次 wave 就出现 3 例，其中 1 例
（T12）直接导致该轮 result 是 `blocked`（尽管交付是完整的）。

**实测到的良好行为（值得保留）**：
- T12（hermes）没有为了「让门禁变绿」去动白名单外的文件，而是**报 blocked 并给出两个最小修法**；
- V9/V10/V12 都把「不是我的、当时正在变红的 target」明确标注为「不可归因于本任务」；
- V11 在自己的测试里用 `#[ignore]` 钉住它发现的缺陷，避免把共享 checkout 的 `cargo test` 弄红。

**建议**：
1. 每个 wave 的 task/verify 模板加一节「门禁同步」：列出**本 wave 之前**的测试里哪些断言可能失效，
   由 controller 在 wave 开始前处理；
2. 或者给测试加「按里程碑打标」的机制（例如 `#[cfg_attr(not(feature="w3"), ignore)]`），
   让过期的 characterization 用例自动退场而不是变红；
3. 明确「controller-owned 测试」的清单，child 一律只报不改（现状是对的，但要有接住的流程）。


## P18（🔴 阻塞，可信度向）「真值」缺了夹具就不是真值

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** `verify.json` 的验收第一条 + known_facts「夹具变更需控制器决定并出新版本」；`task-workflow.md`「Reproduction also runs in a fresh writable copy, never in the stored snapshot; interpreter caches alone can invalidate a stored tree」；F05 的验收也写明「两个变异体不会共享制品」。本文 W3 的 42 格矩阵与 W4 的 mutA/mutB 共用 `CARGO_TARGET_DIR` 两条教训都已进入正式流程。

**现象**：V11（codex，独立验证者）判定 T11（opencode 作者）时报了两个 FAIL：
- FAIL 1：删除路径跟随 symlink 祖先，删掉工作区外的数据 —— 真缺陷，无争议；
- FAIL 2：index 有 `a/b/c`、工作区 `a` 是指向工作区外的 symlink、目标 rev **修改** `a/b/c` 时，
  mg exit 1 拒绝而**真实 git exit 0**（换成真目录并写入）。

controller 复跑 FAIL 2 时把夹具换成了「工作区外那个文件**存在**」，得到 `exit 1 拒绝`，
于是判定「V11 测错了」，并把这个结论写进返工任务书 §3.2 与 V11b 验证书（v1 更正）。

**作者（omp）在轮内自己做了 git 实验后质疑该前提**，controller 再次复跑并做全变量对照，得到真相：

| 夹具：顺 symlink 祖先能否 stat 到目标 | 真实 git（非 force） | 真实 git（force） |
|---|---|---|
| **能**（outside/b/c 存在） | exit 1 拒绝（与内容是否相同无关） | exit 0，换成真目录并写入，工作区外零变动 |
| **不能**（outside 为空） | **exit 0**，换成真目录并写入，工作区外零变动 | 同左 |

也就是说：**V11 的 FAIL 2 没错，controller 的 v1 更正也没错，两边只是夹具不同。**
错误在于双方都只报了「结论」，没报**夹具的完整状态**；而方案的纪律只要求「验证者对作者的说法取证」，
没有任何一环要求「真值断言必须连夹具一起交付、复跑者必须复用同一夹具」。

**为什么算「阻塞」**：
- 这是同一个错误的**两次连续发生**：第一次在验证者侧（欠规范），第二次在 controller 侧（换变量复跑后自信地宣布对方错了）；
- 若作者没有独立质疑，本轮会以「按错误前提改代码」结束，而 V11b 又会在同一格上复现 controller 的夹具、
  判出**假红**——即 verify 环节的两次「独立确认」来自同一条欠规范的真值；
- controller 的仲裁在方案里是**最终判据**（作者只能报 blocked），所以 controller 的错真值没有下游纠错点。

**实测到的良好行为（值得保留）**：
- T11b 的作者**既没有照做也没有直接反驳**，而是把夹具变量做成 42 格矩阵（祖先类型 × 目标动作 × force），
  用平行仓库逐格对拍 —— 把「谁对谁错」变成「在哪一格上对」，争议当场消失；
- 改码后 controller 抽查 5 格（写路径 4 格 + FAIL 1 删除路径 1 格），mg 与 git 在**退出码 / symlink 形态 /
  工作区内内容 / 工作区外 sha256 / porcelain** 上全部逐字节一致。

**建议**（已固化进 W4 的验证任务书）：
1. 「真实 git 会 X」这类断言必须写成 **`夹具状态 → 行为` 的矩阵 + 原始命令与输出**，禁止只写一句结论；
2. 复跑者换任何夹具变量时，必须显式声明「我改了哪个变量、原报告是什么夹具」，否则不许推翻对方结论；
3. 任务包应显式允许 child **质疑任务书的前提**（先报冲突、附实测，再动手），W4 模板已补这条。


## P19（🟡 中等）成本账本没有「轮型」词表

> **上游现状（2026-09-20 核查）：🔴 上游仍未处理。** `usage.py:17` 仍是 `PURPOSES = ("task", "retry", "report_repair")`。上游 F08 待办：「保留 attempt purpose，新增独立 role/phase 和宿主后台活动分类」，并计划用「项目级 / job 级 / round 级汇总 + source coverage + unknown/unattributed」取代现在单薄的 purpose 维度。

**现象**：`usage.py` / `native_usage.py` 的 `purpose` 只接受 `task` / `retry` / `report_repair`
（`usage.PURPOSES`）。而方案的 round 模型里明确有**验证轮**与**返工/复验轮**：
W3 的 6 个 manifest（T9 / T11 / V9 / V11 / T11b / V11b）被迫全部填 `task`，
于是「验证花了多少 token」「返工花了多少」无法从账本里直接算出来（W3 只能手工在 summary 里分列）。

**为什么值得记**：返工率是 §6 的核心指标之一，而返工**成本**同样重要
（W3 的 T11b+V11b 占 wall 时间 28%）——现在只能靠人肉把 manifest 和轮次表对起来。

**建议**：把 `purpose` 扩成 `task / verify / rework / reverify / retry / report_repair`，
或在 manifest 里加一个不影响计数的 `role` 字段。

**实测到的良好行为**：同一 native session 的**续轮是独立的 `turn`（不同 `native_id`）**，
所以「复验轮复用原会话」既能省冷启动，又能把成本精确切开（V11 63 calls / V11b 77 calls 已分别入账）。


## P20（🟡 中等）过期的 `#[ignore]` 标记不会自己失效

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** `task-workflow.md`：「Rework grants explicit ownership of old characterization and ignored tests. Report each replacement/reactivation result; unresolved exclusions stay visible with an owner and closure condition.」——过期的 ignore 有了归属与关闭条件。上游对应 F09。

**现象**：`#[ignore]` 是方案里推荐的「钉住已验证缺陷、又不弄红共享 checkout」的手法（W2/V6、W3/V11 都用了）。
但**缺陷被修好之后，标记不会自己消失**，用例就从绿色集合里静默蒸发了：

| 标记 | 写下时 | 修好于 | 何时被发现/清理 |
|---|---|---|---|
| `tests/verify_diff.rs::cli_space_in_path_tab_padding` | W2/V6 | **W2/T6b**（V6b 复验 PASS） | **W4 收口**（跨了整整两个 wave，C-25） |
| `tests/fsck_gc.rs::clone_of_an_origin_without_tags_succeeds` | W4/T15 | W4/T13b | W4 收口（V13 主动报告） |
| `tests/fsck_gc.rs::push_to_a_bare_remote_updates_and_creates_branches` | W4/T15 | W4/T13 | W4 收口（V13 主动报告） |
| `tests/fsck_gc.rs::e2e_fast_forward_merge_in_a_repository_with_subdirectories` | W4/T15 | W4/T16 | W4 收口（V16 实跑通过） |

**为什么要单独记**：P17 说的是「测试**变红**了没人负责」，P20 是它的反面 —— 测试**变绿**（更准确地说：
被 `#[ignore]` 关掉）也没人负责。两者的根因相同：**测试的所有权与生命周期没有被定义**。
而且这类问题在指标上是「隐形」的：`cargo test` 全绿、`ignored` 计数甚至被当作正常现象。

**建议**：
1. 每个 wave 收口时由 controller 跑一次 `cargo test --offline -- --ignored`，逐条对账：
   **通过 → 摘标记（转常驻回归）；失败 → 更新属性里的理由**（本次 4 通过 / 1 已知分歧）；
2. `#[ignore = "…"]` 的理由字符串里要求写**发现者 + 轮次**，便于事后判断它是否已过期；
3. 把「ignored 计数」纳入 §6 指标并用趋势看（本次清理后全仓只剩 1 个已知分歧标记）。

## P21（🟠 严重，可信度向）并发 wave 里「作者观测到的 peer 缺陷」可能是中间态

> **上游现状（2026-09-20 核查）：🟢 已覆盖。** 「我 build 的是哪一版」现在由快照/摘要回答：F05 的 delivery manifest（逐条哈希 + request digest）、`completion.md:28` 要求 handoff 的 snapshot 必须绑定同一个 job，验证使用独立 build/cache/evidence 目录。上游对应 F05。

**现象**：W4 采用策略 A（共享 checkout、目录互斥）并行跑 T13（codex，传输层）与 T15（omp，fsck/gc 收口）。
T15 在自己的构建上实测到三个「不在我写作用域内」的缺陷（D1 merge 快进 / D2 clone 无 tag / D3 push 拒绝 bare 远端与新建分支），
其中 D1/D2 是真的，**D3 不是**：V13 复核时 `mg push` 到 bare 远端与新建远端分支都已工作 ——
因为 T15 观测时 T13 仍在飞行中，它 build 到的是**中间态**。

**为什么值得单独记**：这不是任何一方的错误 —— 三方都按协议做了正确的事：

- T15 按任务书要求「发现跨模块缺陷就报出来」，附了最小复现与原始输出；
- T13 在自己的白名单内把 push 修好了（它的 result 里 push 到 bare 远端、反向 git clone/push 回写全部有原始输出）；
- V13 用自己的真值复核，发现了 T15 的归因过期。

问题在于**协议里没有任何东西能区分「最终交付缺陷」与「飞行中的中间态」**：
`prepare` 不带制品版本，`result` 不要求制品哈希，`baseline.txt` 只覆盖 wave 起点。
于是「谁在什么时候对哪一版制品下了结论」这件事在证据链上是缺失的 ——
这与 **P13**（谁改了哪个文件）是同一个根因，但后果更重：它会让 controller 派出一轮本不该存在的返工。

**W4 里的正面样本**：V16 自发在 result 开头写死了三个制品的 sha256（`src/cli/merge.rs`、作者/自己的测试文件），
并在结论里声明「判定冻结于此版本」。**这就是缺失的那个原语**，应该固化成验证任务书的强制条款。

**建议**：
1. **验证者**的 result 必须在开头冻结「被验证制品的哈希」（V16 的做法），并在验证期间检测作者是否还在改动同一文件；
2. **作者**报告跨模块缺陷时，必须附**自己 build 的制品哈希**，以及 `cargo` 是否复用了共享 `target/`；
3. `prepare` 的 `request.json` 增加一个可选的 `subject_hashes`：controller 在派轮时把被验证文件的 sha256 写进去，
   让「结论 ↔ 制品」的绑定进入协议，而不是靠 agent 自觉；
4. 收口时 controller 用「最后一次改动 mtime + 哈希」核对每个缺陷的处置版本（W4 已靠 `agent` 侧证据手工做到）。

## P22（🟢 轻微）任务包的门禁清单里没有 `cargo fmt`

> **上游现状（2026-09-20 核查）：🟢 已覆盖（改成显式声明制）。** `task-workflow.md:20`：「Include project fmt, lint, supported-runtime or MSRV checks only when the project requires them」——fmt 进入示例验收清单，由项目显式声明；协议层不硬编码 Rust 命令（F09 的边界）。注意是**按需显式声明**，不是默认必跑。

**现象**：W0 的验收明确写了「`cargo test` 全绿、`clippy -D warnings` 干净、`fmt` 干净」，
但 W1–W4 的任务包在「验收/门禁」一节只列了 `cargo test --offline`、`cargo clippy --offline --all-targets`、
`scripts/check-freeze.sh` —— **没有一轮跑过 `cargo fmt`**。于是 4 个 wave 交付的代码格式一路漂移，
收口时 `cargo fmt --all -- --check` 报 **47 个文件**需要重排（其中只有 1 个冻结文件，纯空白改动）。

**为什么值得记**：这是「门禁清单不完整 → 债务静默累积」的典型。它的代价不是错误，而是**收口时的返工**，
而且**不会自己暴露**：`cargo test` 与 `clippy` 都不会因为空行/换行位置而报错。

**建议**：
1. 任务包与验证包的门禁清单统一加上 `cargo fmt --all -- --check`（agent 自己跑，失败自己修）；
2. controller 在每个 wave 收口时对**全仓**跑一次 `cargo fmt --all`（不仅是本 wave 的写作用域），
   否则历史债务会在最后一个 wave 集中爆发；
3. 若某个文件已被冻结，格式化后必须同步 `FREEZE-v0.md` 的哈希并在修订历史里注明「纯空白，公共 API 零改动」。

**W4 的处置**：C-29，FREEZE → v0.7，`cargo fmt --all -- --check` exit 0，测试与 clippy 复跑无变化。

## P23（🟠 严重，流程向）门禁只在 debug profile 下跑

> **上游现状（2026-09-20 核查）：⬜ 上游不覆盖。** F01–F11 未提及本条；上游自己的回归是 Python unittest，没有 debug/release profile 的概念。方法论上同源的是 **F10**「让测试覆盖真实断点，形成下一轮发布基线」。**本条仍然成立**：本测试床的门禁到今天仍然只跑 debug。

**现象**：W1–W4 的每一份任务书与验证任务书的「门禁」一节都是
`cargo test --offline` / `cargo clippy --offline --all-targets` / `scripts/check-freeze.sh` ——
`cargo test` 默认是 **debug** profile。40 个轮次、17 个验证轮，**没有一次**跑过 `cargo test --release`。

**后果（W4 收口后补测，2026-09-20）**：用 release profile 一跑就暴露了 **P24**（见下）——
`mg` 的 **release 二进制会死循环**，而且**即使不挂起，diff 输出也与真实 git 不一致**。
即：项目交给用户的 `mg`（任何 `--release` 构建）是坏的，而所有门禁都是绿的。

**为什么这套流程抓不到**：debug/release 的差异只有三类（溢出检查、`debug_assert!`、`cfg(debug_assertions)`）。
前两类在「测试全绿」时会被认为无关紧要，第三类几乎没人写 —— 于是「debug 绿 = 功能对」这个隐含假设
从未被检验。**只要门禁加一条 `cargo test --release --offline`，这个缺陷在 W3/T6 那轮就会当场暴露。**

**建议**：
1. 任务书的门禁清单固定写**两条**：`cargo test --offline`（快，迭代用）与
   `cargo test --release --offline`（交付用）；验证者的「必须独立执行的检查」同样要跑 release；
2. 把「release 门禁」列入 wave 收口 checklist（与 P20 的 `--ignored` 对账并列）；
3. 对**发布给用户的二进制**，至少跑一次「release 二进制 vs 真实 git」的差分冒烟。

---

## P24（🔴 阻塞）`debug_assert!` 里带副作用的调用——release 下整套 diff 算法失效

> **上游现状（2026-09-20 核查）：⬜ 不适用（产品缺陷，非编排问题）。** 属 mini-git 自身代码，按用户裁决保持不修；对应的流程教训记在 P23。现场归档与最小复现见 `.orch/artifacts/release-p24/`。

> **W7 更新（2026-09-21）：✅ 已修 + 已交付验收（原先的「不修」裁决被推翻）。** 用户在本轮要求「把 mini-git 做完」，
> 于是 P24 由 **opencode/OMO 作者轮**（round `5248318530`，job `66840d4b`）按最小修法修掉，
> 由 **omp 验证者轮**（round `3ccd1e46`，job `9317e017`）独立复验：release == debug == 真实 git（200 例语料
> 零挂起零差异），修复前制品在**同一套脚手架**上稳定复现 9 例挂起 + 13 例与 git 不同（负对照有分辨力）。
> 控制方另用自己的两份重建制品复跑同一脚手架，得到与验证者逐元素相同的三组数字，再以 4 条显式命令
> （已知正确 / 故意错误要求**具体**失败集合 / 场景全绿 / 制品可复现）完成 `kind=delivery` 验收。
> 修复只动 `src/diff/myers.rs`（sha256 `aafe8464…`），8 处 `debug_assert!(f(&mut x))` 全部改成
> `let v = f(&mut x); debug_assert!(v);`。证据：`.orch/waves/W7/evidence/{t1-acceptance,t1-accepted}.txt`、
> `v2-controller-review.txt`、`rounds/agent-orchestrator-a_u_8vn8/{result,publication}.json`。

**现象**（`src/diff/myers.rs::change_compact`，8 处）：

```rust
while go.end == go.start {
    debug_assert!(group_slide_up(x, &mut g));      // ← 带副作用的真实调用
    debug_assert!(group_previous(xo, &mut go));    // ← 同上
}
```

`group_slide_up` / `group_previous` / `group_next` 都以 `&mut` 修改算法状态。
`debug_assert!` 在 release 下被整条编译掉，于是这些状态推进**从未发生**：

| 位置（行）| debug 行为 | release 行为 |
|---|---|---|
| 906 / 918 / 934 / 935 / 961 / 962 / 970 / 973 | 调用被求值，游标前进 | **什么都不做** |

**后果（实测）**：
- `while go.end == go.start { … }`（第 934–935 行）与 `while g.end > best_shift { … }`（961–962）在 release 下变成**空循环 → 永久挂起**（100% CPU、无输出）；
- 其他情形不挂起，但**变更块的压缩/滑动没有做**，diff 结果与 git 不同。

**实测数据（200 例随机语料，生成器与 `random_scripts_are_valid` 同款）**：

| profile | 死循环 | 输出与 debug 不同 | 输出与真实 git |
|---|---|---|---|
| debug | 0 | — | **全部一致** |
| release | **3 / 200** | **12 / 200** | 不一致（case 35 实证：debug == git，release ≠ git）|

**影响面**：`mg diff`（`diff/unified.rs:180`）、`mg merge` 的三方路径（`merge/three_way.rs:201`）都直接调用 `myers()` →
**release 版的 `mg diff` / `mg merge` 会挂起或给出错误的 diff**。release 测试套件里 `lib` 与 `verify_diff` 两个 target 直接卡死。

**最小复现（CLI）**：

```bash
cd /tmp && rm -rf hang && mkdir hang && cd hang
R=/home/user/Projects/mini-git/target/release/mg      # debug 版换成 target/debug/mg
$R init . && printf 'b\n\n\n\na\n  c\na\na\n  c\n\nd\nb\ne\nd\nb\na\n' > f.txt
$R add f.txt && $R commit -m s
printf '  c\na\n  c\n  c\nd\na\n' > f.txt
timeout 6 $R diff      # release: 超时（exit 124）；debug: 正常输出，与 git 逐字节相同
```

**修法**（一处机械修改，8 个位置）：

```rust
// 之前：副作用被 debug_assert! 吞掉
while go.end == go.start {
    debug_assert!(group_slide_up(x, &mut g));
    debug_assert!(group_previous(xo, &mut go));
}
// 之后：先调用（副作用一定发生），再断言返回值
while go.end == go.start {
    let slid = group_slide_up(x, &mut g);
    let moved = group_previous(xo, &mut go);
    debug_assert!(slid);
    debug_assert!(moved);
}
```

**这条缺陷证明了 P23 的价值**：它不是「不够仔细」，而是**门禁 profile 没覆盖**导致的系统性盲区 ——
4 个 wave 的交叉验证、变异测试、字节级对拍全部通过了，却没人跑过一次 release。

## P26（🟡 中等，成本向）delta 的 reset 路径无条件重写内容缓存

> **上游现状（2026-09-21 第五次核查）：🔴 未处理**（本批新发现）。

**现象**：`recovery-delta.md` 的「Read cost, time and storage」一节写明
*"The content file is rewritten only when its payload changes."*
实测：只要本次调用的 `reset_reason` 非空，`contents.json` **一定**被重写，即使内容逐字节没变。

**证据**（真实 W5 run：2 job / 2 watch / 71 个缓存文件 / 140,906 B）：

| 触发原因 | `reset` | `contents.json` sha256 | `cache_read_bytes` | `cache_write_bytes` |
|---|---|---|---|---|
| `token_mismatch`（丢失输出后重放） | true | `3e4984bb…` → `3e4984bb…` **未变** | 203,595 | **203,595** |
| `token_required`（给了游标没给 `--since`） | true | `3e4984bb…` → `3e4984bb…` **未变** | 203,595 | **203,595** |
| `scope_changed`（换 job 过滤） | true | `3e4984bb…` → `bbae6eaa…` 变了 | 203,595 | 162,047（合理） |
| 无变化续读（对照组） | false | 未变 | 203,595 | **0** |

对照组的 `cache_write_bytes:0` 证明判据本身是能工作的 —— 只有 reset 这条路绕过了它。

**根因**（`recovery_delta.py:243-250` + `:173-186`）：

```python
if reason is not None:
    state = {"version": VERSION, "scope": scope, "delivered": {...}, "pending": {...}, "files": {}}
...
def save_cursor(path, state):
    ...
    if state.get("cache_sha256") != content_hash:      # 新 state 里没有这个键 → 恒不等
        write_atomic(path.with_name("contents.json"), contents)
```

reset 时 `state` 被换成全新字典，**丢掉了 `cache_sha256`**；而 `save_cursor` 的唯一判据就是这个键。
于是「内容没变就不写」的优化在 reset 路径上被结构性绕过。

**影响**：纯成本，无正确性影响。缓存上限 4 MiB，最坏情况每次 reset 多写 4 MiB 并多做一次全量序列化。
更值得在意的是它出现在 `token_mismatch` / 丢失输出这类**本来就要重放全部行**的场景里 ——
消费者此刻已经在为「输出丢了」付一次全量代价，这笔写是叠加上去的。

**建议（一行量级）**：把 reset 前已成功读到的缓存哈希（`load_cursor` 返回的 `contents.json` 哈希）带进新 `state`
的 `cache_sha256`，让 `save_cursor` 自己判断；`cursor_invalid` 那种「缓存整份丢弃」的路径本来读不到，自然仍会重写。

---

## P27（🟢 轻微，文档向）`wait_output.py --after` 必须落在行边界，文档未写

> **上游现状（2026-09-21 第五次核查）：🔴 未处理**（本批新发现）。

`supervision.md` 只写「Save the returned `after` byte offset and `identity`, then pass both on the next call」，
没有说这个偏移**必须**是某一行结束后的位置。

**实测**：对一个 85 B 的日志传 `--after 42`（任意中间位置）→
`{"status":"error","error":"offset is not a line boundary"}`，exit 2，且**不给出任何可用的修正值**。

守卫本身是对的（防止消费者在半行里续读导致行内容错乱），但对一个刚被提示"把上次的 after 传回来"的
agent 来说，唯一能自救的办法是回到 `--after 0` 重扫整个日志 —— 而这一条文档也没写。
建议在 `supervision.md` 的 helper 段落补一句「`after` must be a byte offset returned by a previous call
(always a line boundary); otherwise exit 2 — restart from the last known-good offset or 0」。

---

## P28（🟢 轻微，可用性向）重发已发布 result 的拒绝理由是裸 OSError

> **上游现状（2026-09-21 第五次核查）：🔴 未处理**（本批新发现）。

W5-S1 的场景（作者在已发布之后又发了一次），在新配方下确实被挡住了 —— 但挡它的理由不可读。

**实测**（对已发布的轮次重发同一 result）：

```json
{"error": "[Errno 17] File exists: '/tmp/orch-run-mzz9k4gd/rounds/agent-orchestrator-ozz1_n6p/.result.json.5n56bc02' -> '/tmp/orch-run-mzz9k4gd/rounds/agent-orchestrator-ozz1_n6p/result.json'", "kind": "invalid"}
```

三处小问题：

1. 报的是**临时文件名**（`.result.json.5n56bc02`），不是目标名 —— agent 会去猜「是不是有个残留临时文件要清」；
2. 没有说明这是**预期拒绝**（"a valid response already exists; do not republish"），而 `protocol.md` 把这条列为硬规则；
3. `kind` 标成 `invalid`，而同一文件里 `response job_id does not match this round` 用的是 `invalid`+清晰文案。
   同一类冲突（目标已存在）走的是另一条错误通道。

对照：**行为完全正确**（原 `result.json` sha256 不变、没有被改写成 blocked、`job_id` 不匹配另有清晰报错）。
这纯粹是 W5-S1 那类 agent 在**最需要被告知"停下来"**的时刻拿到了一条最不好读的消息。

**建议**：在 `os.link` 之外先做一次显式 `exists()` 检查，返回
`{"error":"a valid response already exists; keep it and wait for a controller round","kind":"conflict"}`。

---

## P29（🟢 轻微，文档向）`PUBLICATION_ARGV` 只在 `prompt.txt` 里，`prepare` 的 JSON 响应没有

> **上游现状（2026-09-21 第五次核查）：🔴 未处理**（本批新发现）。

B2 的发布配方本身很好用（实测：无 shell、stdin 发布、零候选文件、一次成功），但它的入口只写在给 agent 的
`prompt.txt` 里。`protocol.py prepare` 返回的 JSON 里有 `request_path` / `result_path` / `job_id` / `round_id`，
**没有** `PUBLICATION_ARGV`。

对「控制器自己也要用同一套配方」的场景（例如控制器代发、或自动化脚本复用 prepare 的返回值），
调用方必须去读 `prompt.txt` 再字符串切出 argv —— 而 `prompt.txt` 是给模型看的自然语言文本，
没有版本化契约。把同一份 argv 也放进 prepare 的 JSON
（例如 `publication_argv: [...]`）是零成本的一致性改进。

---

## P30（🟠 严重，产品向 / 由交付验收抓到）测试套件把证据**追加写进仓库**，并因此让仓库无法通过交付验收

> **归属**：mini-git 自身代码（`tests/verify_pack.rs`），不是编排 skill 的缺陷。
> 记在这里是因为**抓到它的机制**属于本方案：`delivery.py verify` 的 `source_unchanged` 检查。

**根因**（`tests/verify_pack.rs:566`）：`scratch_dir()` 用
`Path::new(env!("CARGO_MANIFEST_DIR")).join(".orch/waves/W3/T9-pack/verify-scratch")`，
`record()` 用 `OpenOptions::append` 写进去。于是**每跑一次 `cargo test` 就往仓库里追加一份记录**。

**实测污染**（2026-09-21）：`v9-byte-flips.txt` 43 行 / 去重后 1 行（= 这个集成测试至少跑过 43 次）、
`v9-corpus` 类文件同样 43/1、`v9-corrupt.txt` 301 行 / 去重 91 行、`v9-delta-chains.txt` 324,994 B。
对照：同一目录里**没被测试碰过**的 `v9-final-run.log` / `v9-mutation-summary.txt` 的 mtime 仍是
09-19 21:09/21:10，而被写的 10 个 `v9-*.txt` mtime 全变成 09-21 01:45（= 作者跑门禁的时刻）。

**三重影响**：
1. **历史证据不可逆地被污染** —— 「证据归档」这套做法在这条路径上被自己的测试拆掉了；
2. **交付验收被正确挡住**：验收副本在跑完 `cargo test` 之后多出/改写了 `.orch/**`，
   `source_unchanged: false` → **这个仓库在修好之前无法通过 `kind=delivery` 的验收**；
3. **测试不 hermetic**：结果依赖仓库内的历史状态，换一个干净检出跑，行为不同。

**处置**：W7 的 T2 轮（opencode 作者）把落盘位置移出仓库（默认临时目录、尊重 `TMPDIR`、
允许环境变量指定、路径打到 stderr），并**逐字节保留** `.orch/**` 里已有的 10 个文件作为
「被污染的历史」这一事实的证据。缺陷原文与取证见 `.orch/waves/W7/defects.md` 的 W7-D1。

---

## P31（🟡 中等，可信度向）`[profile.release] debug = 1` → 制品 sha256 跨路径不可比，P21 的「冻结 sha256」会假红

> **上游现状（2026-09-21 第六次核查）：🔴 未处理**（本批新发现；与 P21 同族）。

**现象**（W7/V2 实测）：验证者在自己目录里 `cargo build --release` 重建同一份源码，
得到 `c3e51adc…`，而作者声明的（也是真实的）制品哈希是 `21b5679f…` —— **不一致**。

**验证者把根因钉死的方式**（这是本轮最有价值的一次自证）：
1. `.text` 段逐字节相同：两份制品 `objcopy -O binary --only-section=.text` → 同为 `13aee23e…`；
2. `strip` 后仍差 **40 字节**，恰好等于 4 ×（`/home/user/Projects/mini-git` 29 字符 −
   `/tmp/w7-verify/fixed` 19 字符）；
3. 决定性一步：在第三个目录上用 `RUSTFLAGS=--remap-path-prefix=<新路径>=/home/user/Projects/mini-git`
   重建 → sha256 **逐字节等于**作者的 `21b5679f…`。

**为什么这属于编排问题**：P21 的落地配方是「验证者 result 里冻结制品 sha256」。
但只要作者与验证者**不在同一个目录构建**（正常情况就是不在），这个数字**必然不同**，
于是它既可能被当成「验证失败」，也可能被当成「反正是构建路径差异」而**被无脑洗白** ——
两个方向都伤害可信度。

**建议**：
1. 「冻结制品」时同时冻结 **`.text` 段哈希**（或整个 ELF 去掉 `.comment`/`debug_*` 后再哈希），
   sha256 只作为路径敏感的补充；
2. 提交 `result.json` 时把 `RUSTFLAGS`（尤其 `--remap-path-prefix`）与 `[profile.release] debug` 的取值写进声明；
3. 工具侧：`delivery.py` 的 artifacts 段可以加一个「可解释差异」的槽位，
   让「已知路径敏感」不必每次靠验证者手工自证 40 字节的来源。

---

## P32（🟢 轻微，流程文档向）验证者自建工作目录不可能有开工前基线，而流程要求 `V_BASELINE`

> **上游现状（2026-09-21 第六次核查）：🔴 未处理**（本批新发现）。

**`completion.md` 的要求**：接受验证者自己的 job 之前，
「Before the verifier starts, capture its own cwd with the agreed scope … and retain `V_BASELINE`」。

**实测的不可行**：W7 的验证者 cwd 是 `/tmp/w7-verify` —— **本轮为它新建的**工作目录，
轮次开始前根本不存在。开工前 capture 得到的会是空树，「开工后」的 capture 必然带
`no_baseline` 警告，`actual_diff` / `declaration_comparison` 都是 null。

**文档已经说了不能补**（「Missing a pre-work baseline cannot be repaired by labeling a later snapshot
as the original baseline」），但**没给这类轮次的配方**。本轮控制方的处置是：
保留 `no_baseline` 缺口不修，改用三条替代证据 ——
(a) 交付快照自身的 sha256 冻结（绑定 job/round/request/result）；
(b) 验证者声明 `files_created` 的三个脚手架文件都在快照里；
(c) 验证的**对象**（仓库）用另外的机制核对（`myers.rs` sha256 未变 + `check-freeze.sh`）。

**建议**：在 `delivery.md` / `completion.md` 里加一句分支说明：
「若验证者的 cwd 是本轮新建的 scratch 目录，则 baseline 抓**仓库/被验证对象**的 scope，
scratch 目录按 `no_baseline` 记录并显式声明，不要试图补基线」。

---

## P33（🟢 轻微，可用性/安全向）lease token 会随 `claim` 回复返回并落盘在 job history 里

> **上游现状（2026-09-21 第六次核查）：🔴 未处理**（本批新发现）。

**事实**：
* `jobs.py claim` 的回复是**整个 state**，里面包含 `lease.token`（而 `recover` 有意省略 token）；
* 同一份 state（含 token）**原样落盘**在 `<index>/<job>/<6 位序号>.json`。

**实测后果（本轮的真实操作）**：控制方 `claim` 成功但只打印了 `revision`（token 丢在 stdout 里没存），
随后 `recover` 拿不到 token，而 `claim` 又被「job is owned by a live lease」拒绝 ——
按文档只能**等 30 分钟租约到期**。实际做法是直接读 `jobs/<job>/000000000009.json` 里的
`lease.token` 取回所有权，耗时 0 秒。

**为什么值得记**：这不是「有个漏洞」，而是**契约措辞与实际保证不一致**：
文档强调「never infer ownership」「token returned by claim; never infer」，
但 token 事实上是**索引目录里的明文**，任何能读该目录的进程都能接管。
对本项目的用法（共享 `index` 的进程都是同一个 controller）无害，
但如果有人把它当成排他锁来用，会得到比预期弱得多的保证。

**建议**（二选一，别含糊）：
1. 明确写进参考文档：「token 不是秘密；同一 index 的读者之间是**协作式**所有权，
   token 只防误用不防对手」；或
2. token 只出现在 `claim` 的一次性回复里，history 里存**哈希**（那时也要说明
   『回复丢失 = 必须等租约到期』）。

---

## P34（🟢 轻微，可信度向）验证者脚手架在判定失败时仍然 `exit 0`

> **上游现状（2026-09-21 第六次核查）：🔴 未处理**（本批新发现；属「验收脚本卫生」）。

**现象**：W7 验证者写的 `scenario/run_scenarios.py` 把所有断言收进 `fails` 列表、
打印 `FAIL: …`、把 `failures` 写进 JSON —— 但 `main()` **没有** `sys.exit(1)`。
也就是说：**红的脚手架退出码是 0**。

**为什么这会伤到编排**：`delivery.py verify` 的每条计划命令都以 `exit_code == 0` 为「通过」条件
（并且一旦非零就中止后续命令）。如果控制方的验收计划直接调用这类脚手架而不断言其 JSON，
一次**失败**的验证会被读成**通过** —— 这正是方案反复要防的假绿，只是换了个位置出现。

**本轮的处理**：验收计划的 4 条命令全部是「包一层 python」。跑完脚手架后**解析它的产物 JSON**，
对「必须出现的数字」逐项断言（已知正确用例要 200/200/200 且零挂起；故意错误用例要复现出
**具体的那一组**挂起用例号），而不是靠退出码。

**建议**：把「验收脚本必须用退出码表达结论」写进任务包模板的验收条款里；
`delivery.py verify` 也可以对「命令产出 JSON 但退出码恒为 0」的形态给出提示。

---

## 1. 方案里"好用"的部分（对照，避免只记坏消息）

1. **协议本身是可靠的**：W4 的 12/12 轮次 `validate --check-files` 全部通过（W1–W3 亦为 100%），
   result 身份（job_id / round_id / result 路径）零错配，fake-success 零发生。
2. **写作用域纪律可执行**：`drift.sh` 对比 baseline 就能给出客观的越界判定（W1 越界 0）。
3. **交叉验证（作者 kind ≠ 验证 kind）确实有效**：W1–W4 的 17 个验证者轮次全部真的写了独立测试，
   并且抓到了规格错误（W2/T8 的真值来源写错）、边界缺陷（W1 的 gitlink）、
   安全级缺陷（W3 的 symlink 删除越界）、行为偏差（W4 的 `Accept` 头 / 空远端 clone / merge 守卫过严），以及自己方的假红（环境泄漏）。
   交叉验证是这套方案里**唯一持续产生高价值发现**的机制。
4. **文档化的回退路径是真的能用**：`record/watch` 挂掉时，
   "manual supervision loop" + "startup timeout 配方" 两条都按文档救回来了。
5. **`agent-controlled` 协议的自然语言契约足够强**：agent 会主动披露"已知限制""未覆盖"，
   这在 W1 的 5 份 result 里都出现了。

## 2. 待验证（下一步要测的）

W1–W4 都**没有做**这些项（每一 wave 的预算都花在了「把里程碑推完」上），因此它们仍是空白：

| 项 | 计划 | 状态 |
|---|---|---|
| 嵌套委派（depth 1→2） | 让 omp/opencode/codex 各做一次 depth+1 委派，验证 depth 契约与结果汇总 | **协议内未做**（所有 round 都是 depth=1）；但 W4 收口时发现 **hermes 在 W3/V10 自发起了 1 个 `source=subagent` 的会话**（21:07:45，29 calls / 0.29M input），**没有出现在任何 result 里** —— 这是协议外的自主嵌套委派，既没有 depth 契约也没有结果汇总，属当前编排的**盲区** |
| 后台/异步任务 | opencode 的 plugin 后台任务与 codex 的 background exec 在长轮次里的行为 | 未做 |
| 失败恢复 | 主动注入 S4（中途 kill）与 S8（结果被改写），观察是否会被误判为成功 | 未做 |
| 长上下文压缩 | 让 omp/opencode 跑到 `/compact` 触发点，看轮次身份是否保持 | 未做 |
| insider 修复后回归 | 若用户同意打 P1/P2 的补丁，重跑一个 wave 做对比，量化"工具链可用"带来的差异 | 未做（P1/P2 保持不修，见 C-7/C-8） |
| 制品哈希绑定 | **P21 的最小落地**：`request.json` 加 `subject_hashes`，验证者 result 冻结制品哈希 | 未做（W4 靠 V16 自觉，手工核对） |
| C2 delta 真机复验 | 已做（见 §0.5）：真实 run 上 6 次 delta 调用，零副作用、缓存命中、reset 语义与守卫逐条对齐文档 | **已做**（2026-09-21，无需子 agent） |
| B2 `wait_output` 真机复验 | 已做（见 §0.5）：文件尚不存在即开始等，0.40s 命中且业务进程未退出；部分行/软链/FIFO/超长行/截断/替换全部按文档拒绝 | **已做**（2026-09-21，脚本级；**原生宿主侧**仍待 W6） |

> **测试床的结论**：这套 skill 在「协议正确性」「写作用域纪律」「交叉验证」三点上经过 W1–W4 的 **40 个轮次**（含 17 个验证轮）实测
> 是可用的、可观测的；瓶颈集中在 **P5（codex 原生审批）**、**P15/P16/P19（成本计量）**、
> **P17/P20（测试生命周期）**、**P13/P21（制品归因）** 四组，全部有实测证据与最小修法。

> **上游跟进后重估（2026-09-20）**：四组瓶颈里 **P13/P21（制品归因）与 P17/P20（测试生命周期）已进入正式流程**；
> **P5（codex 原生审批）仍是宿主侧行为**，方案选择接受而不是消除；
> **P15/P16/P19（成本计量）上游仍未处理**。也就是说本项目测出来的「成本计量」这一组到今天依然成立 ——
> 上游 F11（可比较的 token / wall time 实验）把前置条件写成「F07/F08 能标注计量覆盖」，
> 正是 W4 summary §4.1 那种「四个 host 手工核账」的自动化版本。
