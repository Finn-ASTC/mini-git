# V3 —— 独立验证 T3（index DIRC）

你是本轮的**验证者**（kind: codex），不是实现者。作者是另一个 agent（kind: opencode）。
你的任务：**不接受作者的任何自述**，以真实 `git` 二进制为唯一真值，独立判定 T3 是否成立。

## 0. 被验证的对象

- 任务包：`.orch/waves/W1/T3-index/task.md`（先读它）
- 作者的结果：`.orch/rounds/W1/agent-orchestrator-mq3n3zpl/result.json`（只读，**不得修改**）
- 作者允许改的文件：`src/index/dirc.rs`
- 开工前基线：`.orch/waves/W1/T3-index/baseline.txt`

## 1. 你的写作用域（白名单）

- `tests/verify_index.rs`（**新建**，你自己的目标 target）
- `.orch/waves/W1/T3-index/verify-scratch/**`（临时仓库，随便用）
- 你的 result 路径（由 `prompt.txt` 给出）

**禁止修改**任何其它文件，尤其是 `src/**`（只读）、`Cargo.toml`、`Cargo.lock`、
`tests/interop/**`、`tests/verify/main.rs`、`tests/verify_refs.rs`、`tests/verify_worktree.rs`、
`.orch/rounds/**`。发现缺陷**不要代改**，写进结论。

## 2. 必须独立执行的检查（逐项给出原始输出）

### (A) 复跑作者的测试与全量门禁

```bash
cd /home/user/Projects/mini-git
cargo test --offline index::
cargo test --offline
cargo clippy --offline --all-targets     # 期望 0 warning
scripts/check-freeze.sh                  # 期望 drift 0
```

> 本 wave 另有 agent 在改 `src/refs/`、`src/worktree/`。编译错误若出自作者白名单之外的文件，
> 记录并在 `output` 里注明「不可归因于 T3」；等 30 秒重试，不要改别人的文件。

### (B) 独立真值测试（最重要，必须是你自己写的）

在 `tests/verify_index.rs` 里写测试，**零硬编码真值**：真值只能来自运行时启动的真实 `git`
（`tempfile::tempdir()` 隔离）。至少覆盖：

1. **字节级往返**：构造一个覆盖多种情况的索引，`cp` 出 `.git/index` 备份，调用
   `minigit::index::Index::read` → `Index::write`，断言 `.git/index` 与备份**字节完全相等**。
   索引里必须包含：嵌套目录、可执行文件（`chmod +x`）、symlink、UTF-8 文件名、
   以及**路径长度 > 0xFFF 的深路径**（用多层目录构造）。
2. **git 二次确认**：往返之后 `git ls-files --stage` 输出与往返前逐字节相同；
   `git status --porcelain` 仍为空；`git update-index --refresh` 成功。
3. **带扩展的索引**：让 git 自己写出 TREE 扩展（`git add` + 一次 `git status` 后再 `git write-tree`
   → 或直接 `git commit`），确认往返仍然字节相等、`tree_oid` 被正确解析出来。
4. **反例（必须失败）**：
   - 改掉 index 任意一个字节（例如最后一个字节）→ `read` 必须返回 `Error::Corrupt`；
   - 把 version 字段改成 3 → 必须返回 `Error::Unsupported`；
   - 把文件截断到 4 字节 / 12 字节 → 必须返回 `Err`，**绝不能 panic**；
   - `.git/index` 不存在 → `read` 返回空 index 且不报错；写出的空 index 仍能被真实 git 读。
5. **名字长度边界**：构造恰好 4095 与 4096 字节的路径各一条，往返必须保持字节相等。

### (C) 断言真实性检查（防假绿）

- 测试必须真的调用 `minigit::index`（不许把 `dirc.rs` 复制一份到测试里）。
- 不许 `#[ignore]`、不许把断言写成永远为真。
- 明确指出作者的测试里哪些用例是真的在跑被测代码（抽 1 条贴证据）。

## 3. 判定

在 `output` 里给出明确结论 `PASS` / `FAIL`，逐条列「语义 → 命令 → 关键输出 → 是否成立」。
任何 FAIL 给出最小复现。「你怀疑但没覆盖」的项单列，不要算 FAIL。

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
- **原生审批只按「单条」处理**：出现「1 Yes, proceed / 2 don't ask again / 3 No」时选 1，
  **不要选 2**（不做常驻批量授权）。
- **cargo target 目录是共享的**：并发 `cargo test` 会等文件锁，属正常现象。
