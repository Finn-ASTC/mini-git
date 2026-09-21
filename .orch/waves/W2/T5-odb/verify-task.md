# V5 —— 独立验证 T5（odb loose + cat-file）

你是本轮的**验证者**（kind: opencode），不是实现者。作者是 hermes。
**不接受作者自述**，以真实 `git` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W2/T5-odb/task.md`（先读）
- 作者结果：`.orch/rounds/W2/agent-orchestrator-a3w3vequ/result.json`（只读，不得修改）
- 作者可改文件：`src/odb/loose.rs`、`src/cli/cat_file.rs`
- 开工前基线：`.orch/waves/W2/T5-odb/baseline.txt`

## 1. 写作用域（白名单）

- `tests/verify_odb.rs`（**新建**）
- `.orch/waves/W2/T5-odb/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件，尤其 `src/**`（只读）、`Cargo.toml`、`tests/interop/**`、
`tests/verify_*.rs`（别人的）、`.orch/rounds/**`。发现缺陷不要代改，写进结论。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline odb::
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```
（编译错误若出自作者白名单之外，注明「不可归因」并等 30 秒重试。）

### (B) 独立真值测试（`tests/verify_odb.rs`，零硬编码真值）

1. **mg 写 → git 读**：用 `minigit::odb::Odb::write`（或 `loose::write_loose`）写 blob/tree/commit/tag
   各一个，然后调真实 `git cat-file -t/-s/-p`，断言类型/长度/内容一致；
   再跑 `git fsck --no-progress` 必须无报错。
2. **git 写 → mg 读**：临时仓库里 `git hash-object -w` 与 `git commit` 造对象，
   用 `Odb::read` 读回，断言 `(Kind, payload)` 与 `git cat-file` 的结果一致；
   `read_loose_raw` 返回的字节必须等于 `git cat-file <type> <oid>` 的
   **`<type> <size>\0` + payload** 形态。
3. **CLI 端到端**：`env!("CARGO_BIN_EXE_mg")` 调 `mg cat-file -t/-s/-p/-e`，
   与 `git cat-file` 同参数逐字节比较（blob 与 tree 都要；tree 用 `git ls-tree <oid>` 比）。
4. **幂等与原子性**：同一对象写两次 → 内容/mtime 不变、返回同一 oid；
   写入过程不留 `objects/tmp*` 残留；文件权限是 0444（`metadata().permissions().mode() & 0o777`）。
5. **iter_loose**：与 `find .git/objects -type f` 里合法的 loose 对象集合一致。
6. **反例（必须失败）**：把某个 loose 文件的 zlib 流截断 → `Err(Corrupt)`；
   把 payload 改一个字节（保留文件名）→ `Err(Corrupt)`；不存在 oid → `Ok(None)`；
   `mg cat-file -p` 一个不存在的 oid → 非零退出 + stderr 有信息，**不得 panic**。

### (C) 防假绿
- 你的测试必须真的调 `minigit::odb`（不许复制作者代码、不许 `#[ignore]`）。
- 抽 1 条作者的用例，说明它的真值来自真实 git。

## 3. 判定
给出明确 `PASS` / `FAIL`，逐条「语义 → 命令 → 关键输出 → 是否成立」。FAIL 给最小复现。

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

- **tree 的 mode 位数是已知的规格冲突**：`task.md` §3.2 写「5 位 `40000`」，但 §4.6 要求
  `mg cat-file -p <tree>` 与 `git ls-tree` 逐字节一致。实测 git 2.55.0 两者都打印 `040000`
  （`%06o`），5 位的 `40000` 只出现在 tree 载荷里。**真值一律以真实 git 为准**（`040000`），
  不要据此判 FAIL；作者已按 git 处置。
- 本轮另有 T6/T7 作者在改 `src/diff/**`、`src/merge/**`。编译错误若出自这两个目录，
  注明「不可归因于本任务」并等 30 秒重试。
