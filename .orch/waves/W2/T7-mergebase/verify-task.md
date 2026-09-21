# V7 —— 独立验证 T7（merge-base）

你是本轮的**验证者**（kind: omp），不是实现者。作者是 codex。
**不接受作者自述**，以真实 `git merge-base` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W2/T7-mergebase/task.md`（先读）
- 作者结果：`.orch/rounds/W2/agent-orchestrator-ce6l9t1x/result.json`（只读，不得修改）
- 作者可改文件：`src/merge/merge_base.rs`
- 开工前基线：`.orch/waves/W2/T7-mergebase/baseline.txt`

## 1. 写作用域（白名单）

- `tests/verify_mergebase.rs`（**新建**）
- `.orch/waves/W2/T7-mergebase/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件，尤其 `src/**`（只读）、`Cargo.toml`、别人的 `tests/verify_*.rs`。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline merge::
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_mergebase.rs`，零硬编码真值）

用真实 git 在 temp repo 里造 DAG（`git commit-tree` 能精确控制拓扑，推荐），
对每一对 (a,b) 断言：

1. **正确性**：`git merge-base --is-ancestor <mg 结果> <a>` 与 `<b>` 都必须为真（exit 0）。
2. **最优性**：若 `git merge-base --all <a> <b>` 只有一个 oid，则 mg 必须返回它；
   若有多个，则 mg 的结果必须 ∈ 该集合。
3. **无共同祖先**：两个 `git checkout --orphan` 的 root → `Ok(None)`。
4. 覆盖：线性、单次合并、**criss-cross**、`a == b`、a 是 b 的祖先（反之亦然）、
   三方合并（3 个 parent）、**~200 个提交的多分支 DAG（性能：必须秒级返回）**。
5. **反例**：不存在的 oid → `Err`；损坏的 commit 对象 → `Err`，不得 panic。

### (C) 依赖降级（重要）
如果 `src/odb/loose.rs`（T5）本轮未完成，`Odb::read_object` 会返回 `NotImplemented`。
这时**不要**判 FAIL，也不要越界实现 odb；按任务包 §4.2 的约定验收作者提供的
**私有 helper**（按 oid 取 Commit 的内部函数），并在 `output` 里明确写
「端到端（经 Odb）未覆盖，原因：T5 未完成」。

### (D) 防假绿
- 必须真的调 `minigit::merge::merge_base`（或作者的私有 helper，若走降级路径）。
- 做一次变异测试证明断言有鉴别力（**用独立 `CARGO_TARGET_DIR` / 仓库副本**，
  不要改本仓库 `src/**`，不要污染共享 target/）：例如让函数恒返回第一个 parent，
  断言你的「最优性」用例会 FAIL。

## 3. 判定
明确 `PASS` / `FAIL`，逐条「拓扑 → 命令 → 关键输出 → 是否成立」。

## 上报

- result 路径：`prompt.txt` 给出的绝对路径。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 不要贴完整逐字稿；给「命令 + 关键输出 + 结论」。**明确区分**
  「FAIL」「未覆盖（说明原因）」「已确认的已知限制」。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1 实测总结，必须遵守）

- **不要触发任何额外交互**：不启动引导/练习流程，不弹 Question/Ask 等人类输入。
  轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes, proceed / 2 don't ask again / 3 No」时选 1，
  **不要选 2**。
- **不要 export 会被 git 读取的环境变量**（`GIT_AUTHOR_*` / `GIT_COMMITTER_*`）。
  用 `git -c user.name=... -c user.email=...` 或 `env VAR=... git ...`。
- **共享 checkout**：本 wave 另有 3 个 agent 在改别的子目录。编译错误若出自作者白名单之外，
  记录并注明「不可归因于本任务」，等 30 秒重试，不要改别人的文件。
- **共享 `target/`**：并发 `cargo test` 会等文件锁（正常）。**变异测试必须用独立
  `CARGO_TARGET_DIR` 或独立副本**（W1 的 V2 因污染共享缓存返工过）。

---

## Controller 备注（开工前补充，v1）

- 作者自述：**第一版测试用单调时间戳，导致「只按时间取最新共同祖先」的变异体蒙混过关**；
  改成 skew/同时间戳后才检出。请你自己也检查这一点：造 DAG 时把时间戳**故意与拓扑相反**，
  并让 criss-cross 的两个 base **同戳**，否则你的断言可能是恒真的。
- 「多个最佳共同祖先」时 v1 只保证结果 ∈ `git merge-base --all`，**不保证**等于
  `git merge-base`（不带 `--all`）的选择 —— 不要据此判 FAIL。
- 本轮另有 V5/V6 两个验证者只改 `tests/verify_*.rs`。编译错误若出自其它 `tests/verify_*.rs`，
  注明「不可归因于本任务」并等 30 秒重试。
