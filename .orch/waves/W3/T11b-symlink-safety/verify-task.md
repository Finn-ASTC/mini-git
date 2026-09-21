# V11b —— 复验 T11b（物化 symlink 祖先：删除路径越界）

你是**上一轮 V11 的同一个验证者（kind: codex）**，本轮继续。你上一轮判了 **FAIL**，并留下复现
`tests/verify_materialize.rs::symlink_ancestor_removal_must_not_touch_outside_worktree`（`#[ignore]`）。
作者（omp）本轮修了删除路径的 symlink 祖先越界，请你复验。

> **重要：先读 `.orch/waves/W3/T11b-symlink-safety/controller-correction.md`（v2）。**
> 你上一轮的 FAIL 1 与 FAIL 2 **都成立**，但 controller 的第一版"撤销 FAIL 2"更正**是错的**：
> 真实 git 在「顺 symlink 祖先 **stat 得到**目标」时拒绝（exit 1），在「**stat 不到**」时放行并换成真目录（exit 0）。
> 两个测量都对，差别只在夹具。本轮的判定口径 = **带夹具的状态矩阵**，不是单条结论。
> 你自己必须当场把这张矩阵跑出来（不要引用 controller 的表，也不要引用你上一轮的报告）。

## 0. 被验证对象

- 返工任务包：`.orch/waves/W3/T11b-symlink-safety/task.md`（先读）
- **Controller 更正（先读，优先级高于任务包与你的上一轮结论）**：
  `.orch/waves/W3/T11b-symlink-safety/controller-correction.md`
- 上一轮你的报告：`.orch/rounds/W3/agent-orchestrator-mub_9jgp/result.json`（只读）
- 作者结果：**由 controller 在 prompt 里给出的 result 路径**（只读，不得修改）
- 作者可改文件：`src/worktree/materialize.rs`、`src/cli/{branch,switch,checkout,reset}.rs`
- 开工前基线：`.orch/waves/W3/T11b-symlink-safety/baseline.txt`（在 T11b 开工前拍的）

## 1. 写作用域（白名单）

- `tests/verify_materialize.rs`（**你自己的文件**：允许**更新**其中「断言旧行为」的用例）
- `.orch/waves/W3/T11b-symlink-safety/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件：`src/**`（只读）、`Cargo.toml`、`Cargo.lock`、别人的 `tests/**`、`.orch/rounds/**`。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline worktree::
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```
（`cargo test --offline` 里的 `ignored` 项数量要与你上一轮一致或更少；若作者的修复让某个 `#[ignore]` 用例
可以取消 ignore，就在你这一轮取消它。）

### (B) 复验（逐条给「命令 → 原始输出 → 是否成立」）

**判定口径 = controller-correction.md（v2）里的夹具矩阵。** 真值必须由你自己当场用真实 git 复跑得到；
**每一格都要写清夹具状态**（工作区外目录是空的还是有文件），并贴上原始命令与输出。

1. **FAIL 1（安全，本轮核心）**：删除路径遇 symlink 祖先（工作区外有数据），对
   `mg switch` / `mg checkout -f` / `mg reset --hard` 三条命令断言：
   - 工作区外目录树 **sha256 清单（内容 + 路径 + 类型 + 目录项）** 与执行前**逐字节相同**；
   - 与真实 git 同格一致（git：非 force exit 1、symlink 保留；force exit 0、symlink **保留**、outside 零变动、
     `git status --porcelain` 只剩 `?? a`）；
   - 确认 `prune_empty_dirs` 不会把 symlink 指向的目录 `rmdir` 掉。
2. **FAIL 2（成立，但只在「顺 symlink stat 不到目标」这一格）**：
   - 写路径 + symlink 祖先 + **outside 为空**（stat → ENOENT）→ 断言 mg 与 git **都 exit 0**、
     symlink 被换成**工作区内**的真目录、写出的内容在 `canonicalize` 后位于工作区内、工作区外零变动、status 干净；
   - 写路径 + symlink 祖先 + **outside 里目标存在**（stat 成功）→ 断言 mg 与 git **都 exit 1 拒绝**、
     工作区/index/工作区外零改动；
   - 写路径 + symlink 祖先 + **未跟踪的新路径** → 断言 mg 与 git **都拒绝**（git 报
     `untracked working tree files would be overwritten: a`）；
   - 把你上一轮 `symlink_ancestor_write_stays_inside_worktree` 里那条旧断言**更新成上面的矩阵口径**
     （原来它只钉住「拒绝」这一格），并在 result 里说明你改了什么、为什么。
3. **库级守卫不得回退**：直接调 `write_blob_to()` / `ensure_parents()`（symlink 祖先）仍必须返回 `Err`；
   「换成真目录」只允许发生在物化写路径的清场步骤。若作者把库级守卫也放开了 → 判 FAIL。

3. **symlink 祖先指向工作区内目录**的情形：删除与写入各一例，两个方向都要与真实 git 逐字节对拍
   （工作区内容、`ls -l` symlink 目标、`git status --porcelain`）。
4. **不得回退**：把上一轮 16 个 PASS 场景里最容易受影响的三类各重跑一次
   （普通分支切换、`reset --hard` 含本地改动、`checkout <rev> -- <path>`），确认没有行为回退。
5. **变异测试**（独立副本 + 独立 `CARGO_TARGET_DIR`）：至少两个变异体 ——
   ①把「symlink 祖先守卫」删掉（期望你的安全用例 FAIL）；②把「symlink 祖先 → 换成真目录」改回「拒绝」
   （期望对齐 git 的用例 FAIL）。

## 3. 判定
明确 `PASS` / `FAIL`，逐条列证据。若仍 FAIL，给最小复现（要能一行行照着跑）。

## 上报

- result 路径：`prompt.txt` 给出的绝对路径（**本轮是新 round，不要覆盖上一轮的 result**）。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：命令 + 关键输出 + 结论；明确区分「FAIL」「未覆盖」「已知限制」。
  **必须写明你更新了 `tests/verify_materialize.rs` 的哪些断言**。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1–W3 实测总结）

- 不触发任何额外交互；原生审批只按「单条」处理（选 1，不选 2）。
- 不 export `GIT_*`；用 `git -c key=value` / `env VAR=... git ...`。
- 共享 checkout：编译错误若出自你的白名单之外，注明「不可归因于本任务」等 30 秒重试。
- 变异测试必须用独立 `CARGO_TARGET_DIR` 或独立副本；临时仓库放 `tempfile::tempdir()` 或 `/tmp/<你的名字>/`。
