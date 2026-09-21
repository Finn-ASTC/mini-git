# V6b —— 独立复验 T6b（`mg diff` 含空格路径的 TAB 修复）

你是本轮的**验证者**（kind: codex），不是实现者。作者是 omp（T6b 返工轮）。
**不接受作者自述**——上一轮（V6）正是靠真实 git 对拍抓出了这个缺陷，本轮要用同样口径确认它真的被修好。

## 0. 被验证对象

- 返工任务包：`.orch/waves/W2/T6b-diff-tabfix/task.md`（先读）
- 上一轮验证报告：`.orch/rounds/W2/agent-orchestrator-6tv40dmx/result.json`（只读）
- 上一轮留下的回归用例：`tests/verify_diff.rs::cli_space_in_path_tab_padding`（**只读，不要改**）
- 作者可改文件：`src/diff/unified.rs`、`src/cli/diff.rs`
- 开工前基线：`.orch/waves/W2/T6b-diff-tabfix/baseline.txt`

## 1. 写作用域（白名单）

- `tests/verify_diff_paths.rs`（**新建**；不要动 `tests/verify_diff.rs`，它是上一轮验证者的文件）
- `.orch/waves/W2/T6b-diff-tabfix/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件，尤其 `src/**`（只读）、`Cargo.toml`、`Cargo.lock`、
别人的 `tests/verify_*.rs`、`.orch/rounds/**`。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline diff::
cargo test --offline                       # 应全绿（verify_diff 里那条 ignored 仍是 ignored）
cargo clippy --offline --all-targets       # 0 warning
scripts/check-freeze.sh                    # drift 0
cargo test --offline --test verify_diff cli_space_in_path_tab_padding -- --exact --ignored
```
最后一条是上一轮的回归用例：**必须 0 failed**（把它从 ignored 唤起来跑）。贴原始输出。

### (B) 独立真值测试（`tests/verify_diff_paths.rs`，零硬编码真值）

用真实 git 造名字矩阵，逐字节比较 `mg diff` 与 `git diff --no-renames`（以及
`git diff --no-index --no-renames` 的引擎级对拍，如果引擎入口可用）：

1. 名字矩阵：无空格；**含空格**；目录分量含空格；结尾空格；含 `"`；含 `\`；含 TAB；
   CJK；非 UTF-8 字节；名字里含 `→` 等非 ASCII；**名字同时含空格与引号**。
2. 三种调用组合：工作区 vs index、`--staged`、`HEAD` vs 工作区。
3. `-U0` 与 `-U3`。
4. 断言 `diff --git` 行**没有**被补 TAB（回归方向：不要修出新问题）。
5. 断言无空格名字**仍然不补** TAB（防止作者把规则写成「总是补」）。
6. 二进制文件 + 含空格名字（`Binary files ... differ` 行）也要一致。

### (C) 防假绿
- 必须真的调 `mg` 二进制（`env!("CARGO_BIN_EXE_mg")`）与 `minigit::diff`；
  不许把 git 输出直接当被测函数的返回值。
- 做一次**变异测试**（独立仓库副本 + 独立 `CARGO_TARGET_DIR`）：
  把「含空格才补 TAB」改成「从不补」或「总是补」，断言你的测试会 FAIL。

## 3. 判定
明确 `PASS` / `FAIL`，逐条列「场景 → 命令 → 关键输出 → 是否成立」。
另外必须回答：**上一轮报告里的失败用例现在是否真的变绿**（贴命令与输出）。

## 4. 真值来源必须自证
任务书里的命令只是提示。如果你发现某条命令产出的东西不是你要的真值，改用正确的来源并在 result 里说明。

## 上报

- result 路径：`prompt.txt` 给出的绝对路径。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：命令 + 关键输出 + 结论；明确区分「FAIL」「未覆盖」「已知限制」。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1/W2 实测总结）

- 不触发任何额外交互；原生审批只按「单条」处理（选 1，不选 2）。
- 不 export `GIT_*`（用 `git -c key=value` / `env VAR=... git ...`）。
- 共享 checkout：编译错误若出自你的白名单之外，注明「不可归因于本任务」等 30 秒重试。
- 变异测试必须用独立 `CARGO_TARGET_DIR` 或独立副本；临时仓库放 `tempfile::tempdir()` 或 `/tmp/<你的名字>/`。
