# T11b —— 修复物化的 symlink 祖先缺陷（删除越界 + 写路径与 git 不一致）

你是本轮的**实现者**（kind: omp）。这是 T11 的**返工轮**：独立验证者（codex）用平行仓库对拍，
抓到两个真实缺陷，需要你按真实 git 的行为修掉。**只修这两个问题，不要顺手改别的。**

## 0. 缺陷（验证者已复现，直接照做）

**FAIL 1（安全，必须修）：删除路径会跟随 symlink 祖先，删掉工作区以外的数据。**
```
cd /tmp && rm -rf v11-fail1 && mkdir -p v11-fail1/outside/b && cd v11-fail1 && git init -q -b main r && cd r
mkdir -p a/b && echo v1 > a/b/c && git add -A && git -c user.name=t -c user.email=t@e commit -qm one
git -c user.name=t -c user.email=t@e checkout -q -b prune && git rm -q a/b/c && git -c user.name=t -c user.email=t@e commit -qm prune
git -c user.name=t -c user.email=t@e checkout -q main
echo arbitrary > /tmp/v11-fail1/outside/b/c && rm -rf a && ln -s /tmp/v11-fail1/outside a
/home/user/Projects/mini-git/target/debug/mg reset --hard prune
```
实测：mg exit 0，随后 `/tmp/v11-fail1/outside/b/c` 与 `outside/b` **都被删掉**。
真实 git（同场景）：`git switch prune` → exit 1（"local changes would be overwritten"）；
`git checkout -f prune` → exit 0，但**工作区外一字未动、symlink 保持不变**。
根因（只读定位）：`src/worktree/materialize.rs` 的 `remove_worktree_entry`（~500 行）与
`prune_empty_dirs`（~514 行）直接 `fs::remove_file` / `fs::remove_dir`，**没有逐段确认祖先不是 symlink**；
而写路径的 `ensure_parents`（~208 行）有这道守卫。switch / checkout（含 `-f`）/ reset --hard 三条命令全部中招。

**FAIL 2（与 git 不一致，同轮处理）：写路径遇到 symlink 祖先时 mg 更保守，导致与 git 不同。**
- 场景：index 里有 `a/b/c`；工作区里 `a` 是指向工作区外的 symlink；目标 rev **修改** `a/b/c`。
- mg：exit 1 `refusing to overwrite local changes: a`。
- 真实 git：exit 0 —— **把 symlink 换成真目录、在目录里写 `a/b/c`**（工作区外仍零变动，`git status` 干净）。
- 影响：违反 T11 §3.3 的「与真实 git 逐字节一致」硬标准。

## 1. 目标（签名不许改）

| 文件 | 要做的事 |
|---|---|
| `src/worktree/materialize.rs` | ①删除/prune 路径加 symlink 祖先守卫；②写路径对齐 git 的「symlink 祖先 → 换成真目录」行为 |
| `src/cli/{branch,switch,checkout,reset}.rs` | 只有当上述语义需要在调用处配合时才动；**不要**顺手改别的逻辑 |

`src/worktree/mod.rs`、`src/cli/mod.rs` 是 CONTROLLER-OWNED，不许动；两个函数的签名不许改。

## 2. 写作用域（白名单，只有这五个文件）

- `src/worktree/materialize.rs`
- `src/cli/{branch,switch,checkout,reset}.rs`

**不许改**任何其它文件，尤其 `tests/verify_materialize.rs`（那是验证者的文件，只读跑它）、
`tests/verify_worktree.rs`、`tests/interop/**`、`Cargo.toml`、`Cargo.lock`、`.orch/**`。

## 3. 要求

1. **FAIL 1 修法（安全优先）**：删除与 prune 之前，对目标路径的**每一段祖先**做 `symlink_metadata`，
   只要有一段是 symlink（或不存在、或不是目录），就**不要**对该路径执行 unlink/rmdir；
   需要做到「工作区外零改动」，并且与真实 git 的实测行为一致（git 保留该 symlink、不动工作区外数据）。
   跨文件的批量删除里，一个路径被跳过不应影响其它路径。
2. **FAIL 2 修法（对齐 git）**：写路径遇到 symlink 祖先时，按真实 git 的行为**把该 symlink 替换成真目录**
   （先 `remove_file` 掉 symlink，再 `create_dir_all`），然后写入 —— 保证内容仍落在工作区内。
   注意：`force=false` 时的「拒绝覆盖本地改动」判断**不能**因此被削弱（该拒绝的场景仍要拒绝）。
3. **回归验证**：
   - 验证者留下的两个用例必须变绿（**只读跑，不要改**）：
     ```bash
     cargo test --offline --test verify_materialize -- --ignored
     ```
     （其中 `symlink_ancestor_write_stays_inside_worktree` 现在断言的是 mg 的旧保守行为，
     你在 §6 里**说明它为什么需要由验证者在新一轮里更新**，不要自己改。若它因此失败，
     在 result 里明确写「该用例断言的是旧行为，需 V11b 更新」。）
   - 门禁全绿：`cargo test --offline`、`cargo clippy --offline --all-targets`（0 warning）、
     `scripts/check-freeze.sh`（drift 0）。
   - 自己再补 3 组平行仓库对拍（mg vs 真实 git）：①FAIL 1 场景（reset --hard / switch / checkout -f 三条命令）；
     ②FAIL 2 场景（switch / checkout）；③symlink 祖先指向**工作区内**目录的删除与写入。全部要逐字节对拍
     （工作区内容、`ls -l` 的 symlink 目标、`git status --porcelain`、工作区外的 sha256 清单）。
4. 在 `materialize.rs` 的 `#[cfg(test)] mod tests` 里加最小回归用例，钉死这两点（真值来自真实 git 或文件系统）。

## 4. 非目标

- 不实现 reflog、不改 `materialize_tree` 的返回值语义、不重构 `ensure_parents` 的整体结构。
- 不改 `Cargo.toml`；需要新依赖请报 `blocked`。

## 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：命令 + 真实结论（两个 FAIL 的最小复现现在是什么行为、工作区外 sha256 清单是否为零改动、
  门禁数字）；已知限制单列。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1–W3 实测总结）

- 不触发任何额外交互（omp 轮次结束后若弹「学习练习」→ 选「不要」）；原生审批只按「单条」处理。
- 不 export `GIT_*`；用 `git -c key=value` / `env VAR=... git ...`。
- 共享 checkout：V11b 会很快起来验证；编译错误若出自你的白名单之外，注明「不可归因于本任务」等 30 秒重试。
- 变异测试/临时构建用独立 `CARGO_TARGET_DIR` 或独立副本；临时仓库放 `/tmp/<你的名字>/`。
