# V16 —— 独立验证 T16（`mg merge` 嵌套路径快进修复）

你是本轮的**验证者**（kind: omp），不是实现者。作者是 opencode。
**不接受作者自述**，以真实 `git` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W4/T16-merge-ff/task.md`（先读）
- 作者结果：**由 controller 在 prompt 里给出的 result 路径**（只读，不得修改）
- 作者可改文件：`src/cli/merge.rs`、`tests/verify_merge_ff.rs`
- 缺陷背景：T15（omp 作者）在 e2e 里发现 `mg merge` 快进在**含子目录**的仓库里必然失败
  （`fatal: corrupt tree entry name: "dir/b.txt"`）；`tests/fsck_gc.rs` 里有 T15 留下的
  `#[ignore]` 回归用例 `e2e_fast_forward_merge_in_a_repository_with_subdirectories`。

## 1. 写作用域（白名单）

- `tests/verify_merge_ff.rs`（**若作者已建同名文件，你改用 `tests/verify_merge_ff2.rs`**）
- `.orch/waves/W4/T16-merge-ff/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件：`src/**`（只读）、`tests/fsck_gc.rs`（T15 的文件）、
作者的 `tests/verify_merge_ff.rs`（只读）、`Cargo.toml`、`Cargo.lock`、`.orch/rounds/**`。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline merge
cargo test --offline
cargo test --offline --test fsck_gc -- --ignored e2e_fast_forward_merge_in_a_repository_with_subdirectories
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（零硬编码真值）

**不要复用作者的 `/tmp/t16/diff.sh`**：自己写一套平行仓库对拍（同初始态 cp -a 两份，
一份跑 `mg`、一份跑真实 `git`）。至少要覆盖：
1. **核心缺陷**：含子目录的仓库 + 快进（`dir/b.txt` 存在）→ mg 与 git 的
   `HEAD tree (%T)`、`status --porcelain`、`ls-files --stage`、工作区清单（含 0755 与 symlink 目标）、
   `fsck` 输出全部一致；
2. 快进**删除**子目录里的文件、快进**新增多层目录**（`a/b/c/d.txt`）；
3. 快进时工作区**有本地改动**（非 force）→ 断言 mg 与 git 都拒绝且工作区/index/refs 零改动；
4. **非快进**回归：干净三方合并、子目录内文件冲突（冲突码、stage、字节）；
5. `mg pull` 的**非快进**路径（T13 报告说它仍会走进 `fast_forward()`）：
   构造远端前进 + 本地分叉 → 断言 mg 与 git 行为一致（如果 mg 仍失败，那是 T16 未覆盖的路径，
   要报出来但注明「属 T13/T16 的边界，需 controller 决策」）；
6. **不许假绿**：确认测试真的调 `mg` 二进制；真值只能来自真实 git 进程与文件系统。

### (C) 变异测试（独立副本 + 独立 `CARGO_TARGET_DIR`）
至少 2 个变异体：①把修复回退成 `flat_tree(...)`（期望核心用例 FAIL）；②让 `fast_forward` 把
index 更新成错误的 oid（期望对拍用例 FAIL）。两个都必须被你的测试检出，否则判 FAIL。

## 3. 判定
明确 `PASS` / `FAIL`，逐条列「场景 → 命令 → 关键输出 → 是否成立」。FAIL 必须给最小复现。

## 上报

- result 路径：`prompt.txt` 给出的绝对路径（本轮是新 round）。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 给「命令 + 关键输出 + 结论」，明确区分「FAIL」「未覆盖」「已知限制」。

## 真值来源必须自证（W2/W3 教训 P11/P18）

任务书里的「真实 git 会 X」只是提示。**夹具必须写全**（文件是否存在、目录层次、权限位），
否则结论不可复现（P18：同一句话在两个夹具上结论相反）。发现任务书写错 → 在 result 里写明
「任务书 X 处有误，正确的是 Y」，并按实测真值继续。

## 门禁归属（W3 教训 P17）

本 wave 有其它 agent 在同一 checkout 并行改 `src/transport/**`、`src/cli/{clone,fetch,push,pull}.rs`。
不属于你的 target 变红 → 注明「不可归因于本任务」，等 30 秒重试，不要改别人的文件，也不要因此判 FAIL。
另外：`tests/verify_http.rs` 需要 bind 本地端口，在**受限沙箱**里会因 `PermissionDenied` 失败
（controller 侧已实测），这与代码无关 —— 若你遇到，按环境限制记录。

## 补充条款

- **不许「skip 即通过」**：环境缺失时 `return` 而不失败的写法一律视为假绿，点出来。
- **可质疑自己的上一轮**（如果你有）：发现旧结论被夹具误导就直接更正。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律

- 不触发额外交互；omp 轮次结束后若弹「学习练习」→ 选「不要」。原生审批只按「单条」处理（选 1）。
- 不 export `GIT_*`（用 `git -c key=value`）。
- 变异测试必须用独立 `CARGO_TARGET_DIR` 或独立副本；临时仓库放 `tempfile::tempdir()` 或 `/tmp/<你的名字>/`。
