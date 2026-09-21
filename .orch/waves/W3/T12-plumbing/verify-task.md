# V12 —— 独立验证 T12（add/rm/status/commit/log/tag + `T` 类型变化）

你是本轮的**验证者**（kind: opencode），不是实现者。作者是 hermes。
**不接受作者自述**，以真实 `git` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W3/T12-plumbing/task.md`（先读）
- 作者结果：**由 controller 在 prompt 里给出的 result 路径**（只读，不得修改）
- 作者可改文件：`src/cli/{add,rm,status,commit,log,tag}.rs`、`src/worktree/status.rs`
- 开工前基线：`.orch/waves/W3/T12-plumbing/baseline.txt`

## 1. 写作用域（白名单）

- `tests/verify_plumbing.rs`（**新建**）
- `.orch/waves/W3/T12-plumbing/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件，尤其 `src/**`（只读）、`Cargo.toml`、`Cargo.lock`、
别人的 `tests/verify_*.rs`、`.orch/rounds/**`。发现缺陷不要代改，写进结论（要 controller round）。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline cli::
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_plumbing.rs`，零硬编码真值）

1. **逐命令对拍**（至少 3 个「有分叉、有合并、有二进制文件、有非 ASCII 路径」的仓库，
   每个命令都做 `mg` vs `git` 的**进程输出**逐字节比较）：
   - `add`：`git ls-files --stage` 集合与 stage/mode/oid 全等；`mg add .` 与 `git add .`
     一致（含 ignore）；删除检测（工作区删文件后 `add <dir>`）。
   - `rm`：`rm` 与 `rm --cached`；`git status --porcelain` 一致。
   - `status --porcelain`：**逐字节一致**，含非 ASCII/空格路径的引号转义、分组顺序
     （先已跟踪变更、再未跟踪）。
   - `commit`：`git fsck --no-progress` 无 error；`git log --format=raw` 的
     author/committer/tree/parent 行与预期一致；`--amend`、`--allow-empty`、
     无改动时拒绝；`-a`；**冲突状态（`MERGE_HEAD` 存在）下能收尾合并**（两 parent）。
   - `log`：`mg log --oneline` 与 `git log --oneline` 逐字节一致（含合并提交顺序）。
   - `tag`：`git tag -l`、`git cat-file -p refs/tags/<name>`、objecttype 一致；`git fsck` 干净。
2. **`T` 类型变化（本轮必测）**：真实 git 造 `T` 场景（例如
   `git rm --cached f && ln -s target f`），断言 `mg status --porcelain` 与
   `git status --porcelain` **逐字节一致**（不是「都写了 T」就够）。
3. **双向互操作**：mg 写的仓库能被真实 `git status/commit/log/tag` 直接使用；
   真实 git 写的仓库能被 mg 正确读取。两个方向都要有测试。
4. **反例**：`mg rm` 无 `-f` 且内容不一致 → 拒绝且**零文件改动**；
   冲突状态下 `mg commit` → 拒绝；`mg add` 不存在的路径 → 非零退出 + stderr、不 panic。
5. **不许假绿**：必须真的调 `mg` 二进制（`env!("CARGO_BIN_EXE_mg")`）与
   `minigit::{index,refs,worktree,odb}`；真值来自真实 git 进程。

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

- **作者本轮的 result `status` 是 `blocked`，但交付是完整的**：它按任务书 §2 没有越界改两个
  白名单外的过期测试，而是报 blocked 请 controller 裁决。controller 已用一次 round 修掉：
  - `tests/interop/smoke.rs`（W0 断言「`mg status` 必须未实现」）→ 改成「已实现命令必须像 git +
    未知子命令必须响亮失败」（`ORCHESTRATION.md` C-18，`FREEZE-v0.md` v0.5）；
  - `tests/verify_worktree.rs` 的 `deviation_type_change_regular_to_symlink` → 改成
    `type_change_regular_to_symlink_matches_git`（C-19）。
  **这两个文件现在是绿的，不要把它们当成 T12 的缺陷**；也不要再改它们（只读）。
- `mg gc` 仍返回 `not implemented yet: cli::gc (T15)` —— T15 在 W4，**不是** T12 的问题。
- `mg status --porcelain=v2` 报 `NotImplemented` 是任务书允许的已知限制（写进你的「未覆盖」即可）。
- 本 wave 另有 T11 的验证者（codex）在并行跑；编译错误若出自 T11 的文件，注明「不可归因」再重试。
