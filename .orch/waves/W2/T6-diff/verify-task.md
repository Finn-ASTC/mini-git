# V6 —— 独立验证 T6（diff 引擎 + mg diff）

你是本轮的**验证者**（kind: codex），不是实现者。作者是 omp。
**不接受作者自述**，以真实 `git diff` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W2/T6-diff/task.md`（先读）
- 作者结果：`.orch/rounds/W2/agent-orchestrator-tezp4suy/result.json`（只读，不得修改）
- 作者可改文件：`src/diff/myers.rs`、`src/diff/unified.rs`、`src/cli/diff.rs`
- 开工前基线：`.orch/waves/W2/T6-diff/baseline.txt`

## 1. 写作用域（白名单）

- `tests/verify_diff.rs`（**新建**）
- `.orch/waves/W2/T6-diff/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件，尤其 `src/**`（只读）、`Cargo.toml`、`tests/interop/**`、
别人的 `tests/verify_*.rs`、`.orch/rounds/**`。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline diff::
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_diff.rs`，零硬编码真值）

核心手法：在 `tempfile::tempdir()` 里建**真实 git 仓库**，用 git 造出两个版本的同一文件
（或直接 `git diff --no-index`），把 `git diff` 的原始 stdout 作为真值。

1. **引擎级**：调 `minigit::diff::diff_texts(old, new, a_label, b_label, context)`，
   与真实 `git diff` 的「`---`/`+++`/`@@` 及之后的部分」逐字节比较。
   至少覆盖（每个用例独立 temp repo 或独立文件对）：
   - 纯新增文件、纯删除文件、中间插入/删除/替换；
   - 多 hunk、**相邻 hunk 合并**（间隔恰好 2×context 的边界，两侧都测）；
   - `context = 0 / 1 / 3`（`git diff -U0/-U1/-U3`）；
   - 文件末尾无换行（`\ No newline at end of file` 位置必须与 git 一致）；
   - 空文件 ↔ 非空、完全相同、只差一个空行；
   - 含 `\r\n`、含 CJK、含超长单行；
   - 两侧差异集中在文件头/尾（hunk 头起点在 0/1 的边界）。
2. **CLI 端到端**：`env!("CARGO_BIN_EXE_mg")` vs `git diff`（同参数：默认、`--staged`）
   逐字节比较。**若 odb（T5）本轮未完成**，端到端会因 `NotImplemented` 失败 ——
   这时**不要**判 FAIL，改为记录「未覆盖，原因：依赖 T5 未完成」，并确保引擎级对拍充分。
3. **二进制**：内容含 `\0` 的文件 → 必须是 `Binary files ... differ`，不得打印乱码。
4. **反例**：超大但简单的输入（例如 5000 行里改 1 行）必须秒级返回且结果正确；
   不得 panic。

### (C) 防假绿
- 必须真的调 `minigit::diff`；不许把 git 输出直接当作被测函数的返回值回灌。
- 做一次**变异测试**（在**独立 `CARGO_TARGET_DIR` 或仓库副本**上做，**不要**污染共享 target/，
  也**不要**改本仓库的 `src/**`）：例如把 hunk 头的 `,1` 省略逻辑去掉，
  断言你的测试会 FAIL；证明断言不是恒真。

## 3. 判定
明确 `PASS` / `FAIL`，逐条列「场景 → 命令 → 关键输出 → 是否成立」。

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

- **对拍真值请带 `--no-renames`**：v1 明确不做 rename/copy 检测（task §5 非目标），
  默认 `git diff`（`diff.renames=true`）会打印 `rename from/to` + `similarity index`，
  与 `mg diff` 必然不同；这**不是** FAIL。
- 若发现除 rename 之外仍有逐字节差异，才是真 FAIL。
- 本轮另有 T5/T7 作者在改 `src/odb/**`、`src/merge/**`。编译错误若出自这两个目录，
  注明「不可归因于本任务」并等 30 秒重试。
