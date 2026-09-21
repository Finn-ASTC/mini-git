# V11 —— 独立验证 T11（物化 + branch/switch/checkout/reset）

你是本轮的**验证者**（kind: codex），不是实现者。作者是 omp。
**不接受作者自述**，以真实 `git` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W3/T11-materialize/task.md`（先读）
- 作者结果：**由 controller 在 prompt 里给出的 result 路径**（只读，不得修改）
- 作者可改文件：`src/worktree/materialize.rs`、`src/cli/{branch,switch,checkout,reset}.rs`
- 开工前基线：`.orch/waves/W3/T11-materialize/baseline.txt`

## 1. 写作用域（白名单）

- `tests/verify_materialize.rs`（**新建**）
- `.orch/waves/W3/T11-materialize/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件，尤其 `src/**`（只读）、`Cargo.toml`、`Cargo.lock`、
别人的 `tests/verify_*.rs`、`.orch/rounds/**`。发现缺陷不要代改，写进结论（要 controller round）。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline worktree::
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_materialize.rs`，零硬编码真值）

**平行仓库对拍**：每个场景 `cp -r` 两份（A 跑 `mg`、B 跑真实 `git`，同样命令序列），比较：
工作区文件集合与内容（含可执行位、symlink 目标）、`git ls-files --stage`、
`git status --porcelain`、`git rev-parse HEAD`、`.git/HEAD`。至少覆盖：
1. 普通分支切换（多目录、可执行文件、symlink、空文件、**非 UTF-8 文件名**）；
2. `switch -c`（建→切回→再切走）；
3. detached checkout（`--detach`）；
4. `checkout <rev> -- <path>`（单文件、目录路径）；
5. `reset --soft/--mixed/--hard` 三档，各含「有本地改动」的情形；
6. **拒绝场景**：有会被覆盖的本地改动时必须拒绝，且用 `sha256sum` 证明**工作区一字未动**；
7. **删除语义**：rev2 删掉的已跟踪文件必须消失，而**未跟踪**与 **ignored** 文件必须还在；
8. **symlink 安全**：树里存在指向工作区外的 symlink 时，物化其子路径**不得**写到工作区外
   （用 `canonicalize` 断言）。
9. **反例**：不存在的分支/rev/oid → 对应错误类型，不许 panic、不许留半成品
   （失败后 `git status --porcelain` 与失败前一致）。
10. **不许假绿**：必须真的调 `minigit::worktree::materialize` 与 `mg` 二进制。

## 3. 判定
明确 `PASS` / `FAIL`，逐条列「场景 → 命令 → 关键输出 → 是否成立」。FAIL 必须给最小复现。

## 4. 真值来源必须自证（W2 教训 P11）
任务书里写的**具体命令只是提示**，不保证它真的是该真值的来源。开工前你必须自己确认：
例如 `git ls-remote` 的 stdout **不是** pkt-line；`git status --porcelain` 的分组顺序不是全局字节序。
如果你发现任务书给的来源是错的，**在 result 里写明「任务书 X 处有误，正确的是 Y」**，
并改用正确的来源继续验证。

## 5. 变异测试（本轮**所有**验证者都要做，不再是可选）
在**独立仓库副本 + 独立 `CARGO_TARGET_DIR`**（例如 `/tmp/vN-mutant`、`/tmp/vN-target`）上做，
**不要**改本仓库的 `src/**`、不要污染共享 `target/`。至少 2 个变异体，且必须包括：
1. 一个「把被测语义改错」的变异（例如去掉一条上界检查、颠倒一个条件）；
2. 一个「让实现恒返回某个平凡值」的变异。
断言：你的测试**必须 FAIL**（把失败用例名与关键报错贴进 result）。若某个变异体没被检出，
说明该断言是恒真的，**必须补测试**后再判 PASS。

## 上报

- result 路径：`prompt.txt` 给出的绝对路径。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 不要贴完整逐字稿；给「命令 + 关键输出 + 结论」。**明确区分**
  「FAIL」「未覆盖（说明原因）」「已确认的已知限制」。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1/W2 实测总结，必须遵守）

- **不要触发任何额外交互**：不启动引导/练习流程，不弹 Question/Ask 等人类输入。
  轮次结束的唯一标志是 result 文件写完。（omp 轮次结束后默认会弹「学习练习」→ 选「不要」。）
- **原生审批只按「单条」处理**：出现「1 Yes, proceed / 2 don't ask again / 3 No」时选 1，
  **不要选 2**。
- **不要 export 会被 git 读取的环境变量**（`GIT_AUTHOR_*` / `GIT_COMMITTER_*` / `GIT_CONFIG_*`）。
  用 `git -c user.name=... -c user.email=...` 或 `env VAR=... git ...`。
- **共享 checkout**：本 wave 另有 3 个验证者/作者在改别的子目录。编译错误若出自你白名单之外，
  记录并注明「不可归因于本任务」，等 30 秒重试，不要改别人的文件。
- **共享 `target/`**：并发 `cargo test` 会等文件锁（正常）。**变异测试必须用独立
  `CARGO_TARGET_DIR` 或独立副本**（W1 的 V2 因污染共享缓存返工过）。
- 对拍用的临时仓库全部放 `tempfile::tempdir()` 或 `/tmp/<你的名字>/`。

---

## Controller 备注（开工前补充，W3）

- `mg switch -c foo`（不给起点）会**被 clap 拒绝**：`Command::Switch { name: String }` 是必填 positional，
  而 `src/cli/mod.rs` 是 CONTROLLER-OWNED，T11 无权改成 `Option`。**不要据此判 FAIL**；
  用 `mg switch -c foo main` 这个形式验证（与真实 `git switch -c foo main` 等价）。
- T11 本轮把对拍 harness 放在 `/tmp/w3-t11-omp/`（白名单只允许 5 个文件），
  你要用自己的 `tests/verify_materialize.rs` 独立重做，不要引用它的 harness。
- 本 wave 另有 T12（`src/cli/{add,rm,status,commit,log,tag}.rs`、`src/worktree/status.rs`）在并行收尾；
  编译错误若出自那些文件，注明「不可归因于本任务」并等 30 秒重试。
