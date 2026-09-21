# W1 编排 skill 缺陷记录（controller 观察）

> 规则：每条缺陷必须有「现象 → 证据路径 → 最小复现 → 影响 → 建议修复」。
> 这些是**测试床的产出**，不是要立刻修的 bug；修不修由用户决定。

## D1 —— `protocol.py record` 无法记录长生命周期会话里的 insider 资源（阻塞级）

- **现象**：在用户既有的 `default` herdr 会话里用 `--no-focus` 建了 3 个可见 workspace
  （`wN` / `wP` / `wQ`，pane `wN:p1` 等），`record --mode insider` 全部被拒：
  ```
  {"error": "herdr requires an exact pane ID such as w1:p2, not a current/focused alias",
   "kind": "invalid"}
  ```
  但 `wN:p1` **就是** herdr 原样返回的精确 pane ID（见下方证据）。
- **证据**：
  - `herdr --session default workspace create --cwd <project> --no-focus` 的原始返回：
    `"root_pane":{"pane_id":"wN:p1","tab_id":"wN:t1","workspace_id":"wN"}`；
  - `herdr --session default agent start ... --pane wN:p1` 返回 `interactive_ready: true`（证明 ID 有效）；
  - `record` 报错见本节「现象」。
- **根因**：`protocol.py` 的 `validate_resources()`（约第 329 行）用
  `re.fullmatch(r"w[0-9]+:p[0-9]+", pane)` 校验 pane。该正则只覆盖**新建会话**的 ID 形态
  （如 isolated 会话 `orch-w1-t1` 里的 `w1:p1`），而 herdr 在任何**既存/长生命周期会话**里
  按 base-26 风格分配 workspace ID，于是是 `wN` 这种字母形态。
  结论：**insider 模式（尤其是「在用户会话里可见」这一最常用用法）在当前版本下被协议工具链阻断**；
  只有 isolated 模式（新建会话 → `w1`、`w2`…）能通过校验。
- **最小复现**：
  ```bash
  herdr --session default workspace create --cwd /home/user/Projects/mini-git --no-focus
  # 读回 .result.root_pane.pane_id = wN:p1（字母型）
  python3 ~/.agents/skills/agent-orchestrator/scripts/protocol.py record \
    --request <round>/request.json --mode insider --session default \
    --agent x --pane wN:p1 --workspace wN --tab wN:t1 \
    --parent-pane wJ:p1 --parent-tab wJ:t1 --owns-agent --owns-pane --owns-workspace
  # → invalid: herdr requires an exact pane ID such as w1:p2
  ```
- **影响**（本 wave 的实际后果）：
  1. `record` 不可用 → `resources.json` 由 controller **手工按同 schema 落盘**（附 `record_note`）；
  2. `cleanup-plan` 不可用（它也走 `validate_resources`）→ 收尾必须手工核验并按 target 退出命令处理；
  3. `prepare --previous`（follow-up 轮次）会读上一轮的 `resources.json` 并再次校验 → 同样被拒，
     W1 若需返工轮次，只能改用「全新 prepare」并在任务包里显式带上 job 上下文。
- **建议修复**（一行）：把 pane 正则放宽为 `r"w[A-Za-z0-9]+:p[0-9]+"`，
  workspace/tab 同理放宽（`w[A-Za-z0-9]+`、`w[A-Za-z0-9]+:t[0-9]+`），
  或者直接改用 herdr 自身的 ID 语法校验（非空 + 无控制字符 + 含 `:`）。

## D2 —— `watch.py` 对持续重绘 TUI 的噪声（W1 试运行已记录，T1 summary §3.4）

- 详见 `.orch/waves/W1/T1-object/summary.md` §3.4：8 次 `review_due` 中 7 次是纯噪声。
- 本 wave 处理：只对关键词/状态变化事件做人工读屏，`review_due` 降级为「采样」而非「必须处理」。

## D3 —— hermes 轮次结束后自动「self-improvement」，改动其私有 skill 仓库（超出 cwd，不可见）

- **现象**：hermes（T2 作者）在发布 result（19:35:25）之后，于 19:36:16–19:36:23 自动触发
  「Self-improvement review」，对**已存在的** skill `contract-driven-module-delivery` 执行
  `patch` + `write_file`，写入 `~/.hermes/skills/contract-driven-module-delivery/`
  （新增 `references/reference-binary-oracle.md`，改写 `SKILL.md`），并把改动前的内容存进
  `~/.hermes/.curator_backups/blobs/**`。
- **证据**：
  - 屏幕：`💾 Self-improvement review: Skill 'contract-driven-module-delivery' patched ·
    Skill 'contract-driven-module-delivery' written (references/reference-binary-oracle.md)`
    （panes `wN:p1`）；
  - 账本：`~/.hermes/skills/.curator_ledger.jsonl` 三条记录
    （`actor: curator, action: patch|write_file, evidence.session_id: 20260919_193025_5a38f3`，
    各带 before/after sha256）；
  - 落盘时间与 mtime 一致：`~/.hermes/skills/contract-driven-module-delivery/SKILL.md`（19:36，12088 字节）。
- **为什么算问题**：这在协议意义上**完全不可见** —— 它的 result 只报了
  `files_modified: ["src/refs/store.rs"]`，因为契约要求路径相对 cwd。
  一个「只在自己工作区内写代码」的任务，实际产生了 cwd 之外的持久副作用。
  对本测试床的影响是可控的（它动的是 hermes 私有 skill，不是用户共享的 `~/.agents/skills`，
  也不是项目文件），但对**任何以「写作用域」为硬约束的编排**都必须算越界，
  且监督方在只看 result 文件时无法发现。
- **建议**：
  1. 任务包显式声明「不要在工作区外产生持久副作用」并让 agent 在 `output` 里披露；
  2. 监督方在 hermes 轮次结束后固定检查 `~/.hermes/skills/.curator_ledger.jsonl` 的
     `evidence.session_id` 是否等于本轮 session；
  3. 若不允许，需在 hermes 侧关闭 curator（本轮未做，以保留真实行为证据）。

## D4 —— `agent start` 在既存会话中偶发「超时但实际只差一个回车」（复现已确认，配方有效）

- **现象**：为 V3 起 codex 时 `herdr agent start ... --pane wS:p1` 报
  `{"error":{"code":"timeout","message":"timed out waiting for agent startup"}}`；
  但 `pane process-info` 显示前台进程只有 `/usr/bin/fish`，屏幕上是已输入未执行的 `codex`。
- **处理（按 transports.md 的「Startup timeout with a command still in the shell」配方）**：
  `pane send-text wS:p1 $'\r'` 一次 → codex 正常启动 → 因 `agent start` 已失败、
  agent 未注册，用 `agent rename wS:p1 orch-codex-v3` 绑定 → `agent get` 确认。
- **结论**：配方有效（**1 次恢复即成功，未重试 `agent start`**）。但注意这是个**静默的重复启动陷阱**：
  如果监督方不看进程表就重跑 `agent start`，会在这个 TUI 里输入第二份启动命令。
  该缺陷只影响「既存会话」（T1 的 isolated 新建会话未出现）。

## D5 —— 观察到的跨 kind 行为差异（非缺陷，用于横向对比）

| 维度 | hermes | opencode(OMO/Sisyphus) | codex | omp |
|---|---|---|---|---|
| 输出语言 | 中文 | 英文 | 中文 | 中文 |
| 结束态 `agent get` | `done` | `done` | `done` | `idle`（`screen_detection_skipped=true`）|
| 原生审批 | 无 | 无 | 有（复合 bash 逐条弹，含 2 选项版）| 无 |
| 轮次后自动行为 | **self-improvement 改私有 skill（D3）** | 未见 | 未见 | 弹产品级「学习练习」Ask（T1 观察到）|
| 主动报告范围审计 | 有（引用 baseline 对拍）| 有（引用 baseline 对拍）| 待观察 | 有 |

### D1 补充证据：`cleanup-plan` 同样被阻断（第三次复现）

```
$ python3 ~/.agents/skills/agent-orchestrator/scripts/protocol.py cleanup-plan \
    --request .orch/rounds/W1/agent-orchestrator-dbup2s32/request.json
{"error": "herdr requires an exact pane ID such as w1:p2, not a current/focused alias", "kind": "invalid"}
```

即：**prepare/validate 可用，record/watch/cleanup-plan 三个 insider 模式必需的命令全部不可用**。
W1 的收尾因此改为手工核验 + 按 target 各自的退出命令处理（见 summary.md §5）。
