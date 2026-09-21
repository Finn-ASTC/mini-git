# V4 —— 独立验证 T4（worktree scan / ignore / status）

你是本轮的**验证者**（kind: hermes），不是实现者。作者是另一个 agent（kind: codex）。
你的任务：**不接受作者的任何自述**，以真实 `git` 二进制为唯一真值，独立判定 T4 是否成立。

## 0. 被验证的对象

- 任务包：`.orch/waves/W1/T4-worktree/task.md`（先读它；§3 有 controller 对
  「`head_tree` 是已展平 tree」的接口澄清，你的测试必须按这个语义构造输入）
- 作者的结果：`.orch/rounds/W1/agent-orchestrator-dcbrrli0/result.json`（只读，**不得修改**）
- 作者允许改的文件：`src/worktree/scan.rs`、`src/worktree/ignore.rs`、`src/worktree/status.rs`
  （**不含** `src/worktree/materialize.rs` 和 `src/worktree/mod.rs`）
- 开工前基线：`.orch/waves/W1/T4-worktree/baseline.txt`

## 1. 你的写作用域（白名单）

- `tests/verify_worktree.rs`（**新建**，你自己的 target）
- `.orch/waves/W1/T4-worktree/verify-scratch/**`（临时仓库，随便用）
- 你的 result 路径（由 `prompt.txt` 给出）

**禁止修改**任何其它文件，尤其是 `src/**`（只读）、`Cargo.toml`、`Cargo.lock`、
`tests/interop/**`、`tests/verify/main.rs`、`tests/verify_refs.rs`、`tests/verify_index.rs`、
`.orch/rounds/**`。发现缺陷不要代改，写进结论。

## 2. 必须独立执行的检查（逐项给出原始输出）

### (A) 复跑作者的测试与全量门禁

```bash
cd /home/user/Projects/mini-git
cargo test --offline worktree::
cargo test --offline
cargo clippy --offline --all-targets     # 期望 0 warning
scripts/check-freeze.sh                  # 期望 drift 0
```

> 本 wave 另有 agent 在改 `src/refs/`、`src/index/`。编译错误若出自作者白名单之外的文件，
> 记录并在 `output` 里注明「不可归因于 T4」；等 30 秒重试，不要改别人的文件。

### (B) 独立真值测试（最重要，必须是你自己写的）

在 `tests/verify_worktree.rs` 里写测试，**零硬编码真值**：期望值只能来自运行时调用的真实 `git`
（`tempfile::tempdir()` 隔离）。核心手法是「**输入也来自 git**」：

- `head_tree`：解析 `git ls-tree -r -z HEAD`，构造**展平**的 `minigit::object::Tree`
  （entry 的 `name` 是仓库相对全路径，oid 用 `Oid::from_hex`，mode 用 `FileMode::from_bytes`）。
- `index`：解析 `git ls-files --stage -z`，构造 `minigit::index::Index`
  （注意 stage 字段；mode 是 6 位八进制字符串如 `100644`）。
- 真值：`git status --porcelain` 的原始 stdout。

必须覆盖的场景（每个场景独立一个 temp repo）：

1. **已提交文件的修改 / 删除**（工作区改了内容、删了文件）；
2. **stage 过的修改**（`git add` 后 index 与 HEAD 不同 → X 列）；
3. **只 stage 的文件**（`git add` 新文件 → `A `）；
4. **未跟踪文件与未跟踪目录**（目录里全是未跟踪文件 → 必须折叠成一条 `?? dir/`）；
5. **`.gitignore`**：被忽略的文件不出现；`!` 取反救回；`build/` 目录规则；
6. **可执行位变化**（`chmod +x` → ` M`）；
7. **symlink**（`ln -s`，symlink 指向改变算 `M`）；
8. **无提交的仓库**（`head_tree = None`）；
9. **冲突态**（用真实 git 造出 `UU`：两边改同一文件后 `git merge` 留冲突，
   再解析 `git ls-files --stage` 拿到 stage 1/2/3，与 `git status --porcelain` 比对）。

对每个场景断言 `StatusReport::porcelain()` == `git status --porcelain` 原始输出（逐字节）。

反例/边界（必须验证）：
- 空仓库（无 HEAD、无文件）→ `porcelain()` 是空字符串；
- `.gitignore` 里有非法/怪异模式（如 `[`）→ 不得 panic；
- 巨大但稀疏的目录树（例如 3 层嵌套 × 每层若干文件）→ 排序必须与 git 一致。

### (C) 断言真实性检查（防假绿）

- 测试必须真的调用 `minigit::worktree`（不许在测试里重新实现一份 status 逻辑）。
- 不许 `#[ignore]`、不许空断言、不许把 git 的输出直接当作被测函数的输出回灌。
- 抽 1 条作者的测试，说明它的真值来自 git 而不是手写。

## 3. 判定

在 `output` 里给出明确结论 `PASS` / `FAIL`，逐条列「场景 → 命令 → 关键输出 → 是否成立」。
FAIL 必须给最小复现。「怀疑但未覆盖」的项单列。

## 4. 上报

- result 路径：`prompt.txt` 给出的绝对路径。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 不要贴完整逐字稿。

## 5. 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 6. 交互纪律（控制器实测总结，必须遵守）

- **不要触发任何额外交互**：不启动引导/练习流程，不弹 Question/Ask 等人类输入。
  轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes / 2 don't ask again / 3 No」时选 1，不要选 2。
- **cargo target 目录是共享的**：并发 `cargo test` 会等文件锁，属正常现象。
