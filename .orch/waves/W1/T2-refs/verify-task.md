# V2 —— 独立验证 T2（refs 层）

你是本轮的**验证者**（kind: omp），不是实现者。作者是另一个 agent（kind: hermes）。
你的任务：**不接受作者的任何自述**，以真实 `git` 二进制为唯一真值，独立判定 T2 是否成立。

## 0. 被验证的对象

- 任务包：`.orch/waves/W1/T2-refs/task.md`（先读它，里面有本轮要求的全部语义）
- 作者的结果：`.orch/rounds/W1/agent-orchestrator-dbup2s32/result.json`（只读，**不得修改**）
- 作者允许改的文件：`src/refs/store.rs`
- 开工前基线：`.orch/waves/W1/T2-refs/baseline.txt`（越界检查用）

## 1. 你的写作用域（白名单）

- `tests/verify_refs.rs`（**新建**，这是你自己的独立测试 target；不要动 `tests/verify/main.rs`
  ——那是 V1 的文件，也不要在 `tests/verify/` 下新建文件）
- `.orch/waves/W1/T2-refs/verify-scratch/**`（你自己的临时仓库，随便用）
- 你的 result 路径（由 `prompt.txt` 给出）

**禁止修改**任何其它文件，尤其是 `src/**`（那是作者的产物，你只读）、`Cargo.toml`、
`Cargo.lock`、`tests/interop/**`、`tests/verify/main.rs`、`.orch/rounds/**`、
其他任务的 `task.md` / `verify-task.md`。若你认为作者代码有缺陷，**不要替他改**：
写进结论，由 controller 决定是否开新轮次。

## 2. 必须独立执行的检查（逐项给出原始输出）

### (A) 复跑作者的测试与全量门禁

```bash
cd /home/user/Projects/mini-git
cargo test --offline refs::
cargo test --offline
cargo clippy --offline --all-targets     # 期望 0 warning
scripts/check-freeze.sh                  # 期望 checked 21 file(s), drift 0
```

> 注意：本 wave 另有 agent 在改 `src/index/`、`src/worktree/`。如果编译错误出现在
> **不在作者白名单里**的文件上，记录下来并在 `output` 里注明「不可归因于 T2」，
> 等 30 秒重试；不要改别人的文件。

### (B) 独立真值测试（最重要，必须是你自己写的）

在 `tests/verify_refs.rs` 里写测试，**零硬编码真值**：期望值必须来自运行时调用的真实 `git`
（`std::process::Command::new("git")`，用 `tempfile::tempdir()` 隔离），不许手抄 oid/字符串。
至少覆盖：

1. **读方向**：`git init` + 至少 2 个分支 + 1 个带注解 tag + 1 个轻量 tag + 1 次提交，
   然后 `git pack-refs --all` 让一部分 refs 变成 packed。断言：
   - `RefStore::list()` 的集合与 `git show-ref` 的解析结果**逐条相同**（名字 + oid，且已排序）；
   - `RefStore::branches()` 与 `git for-each-ref --format='%(refname:short)' refs/heads/` 相同；
   - `RefStore::packed()` 非空且是 `list()` 的子集；
   - `read_head()` 与 `git symbolic-ref HEAD` 一致。
2. **写方向**：用 `RefStore::update` 造一个新分支，然后让**真实 git** 去读它
   （`git rev-parse <branch>` 必须等于预期 oid；`git branch` 必须列出它）。
   `set_head_detached` 后 `git symbolic-ref HEAD` 必须失败且 `git rev-parse HEAD` 等于该 oid。
3. **CAS**：用错误 `expected` 更新必须返回 `Error::RefConflict`，**并且磁盘上的值不变**
   （用 `git rev-parse` 验证）；对已存在的 ref 用 `expected=None` 创建必须冲突。
4. **delete 与 packed 的交互**：先 `git pack-refs --all`，再 `delete` 一个分支，
   然后 `git show-ref` 必须看不到它（这是本轮最容易漏的坑）。
5. **损坏输入**：把 `.git/HEAD` 写成乱码 → 必须 `Err(Corrupt)` 而不是 panic；
   把 `.git/HEAD` 删掉 → 必须 `Err(RefNotFound)`。

### (C) 断言真实性检查（防假绿）

- 你的测试必须**真的调用被测代码**（`minigit::refs::RefStore` / `minigit::repo::Repo`），
  不能只是断言你自己构造的数据；不许 `#[ignore]`、不许 `assert!(true)` 式空断言。
- 随机抽 1 条作者的测试，把它断言的真值来源贴出来（证明真值来自 git 而不是作者手写）。

## 3. 判定

在 result 的 `output` 里给出明确结论：`PASS` 或 `FAIL`，并逐条列出
「被验证的语义 → 命令 → 原始输出摘要 → 是否成立」。任何一条 FAIL 都要给出最小复现。
如果作者有**未被测试覆盖但你怀疑有问题**的地方，也要写出来（标为「未覆盖」而不是 FAIL）。

## 4. 上报

- result 路径：`prompt.txt` 给出的绝对路径。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 不要贴完整逐字稿；给「命令 + 关键输出 + 结论」。

## 5. 深度与上限

- 本轮的 depth：1；`max_depth`：3。
- 若要再往下委派，必须显式写 `--parent-depth 1`，不得用 shell 变量默认值。

## 6. 交互纪律（控制器实测总结，必须遵守）

- **不要触发任何额外交互**：不启动「学习练习 / tutorial / 引导流程 / 交互式向导」，
  不弹 Question/Ask 对话框等待人类输入。轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes / 2 don't ask again / 3 No」时选 1，不要选 2。
- **cargo target 目录是共享的**：并发 `cargo test` 会等文件锁，属正常现象。
