# T16 —— 修复 `mg merge` 的嵌套路径快进（T15 发现的跨模块缺陷 D1）

你是本轮的**实现者**（kind: opencode）。仓库 `/home/user/Projects/mini-git` 是「与真实 git 互操作」
的 git 子集（CLI 名 `mg`）。本轮只修**一个**缺陷，不要顺手改别的。

## 0. 缺陷（T15 的 omp 作者发现，controller 已独立复现）

```
mg init .; mkdir dir; echo beta > dir/b.txt; mg add .; mg commit -m c1
mg switch -c side main; echo s > s.txt; mg add s.txt; mg commit -m c2
mg switch main; mg merge side
  → mg: fatal: corrupt tree entry name: "dir/b.txt"        (exit 1)
```
真实 git 同场景：`git merge side` → exit 0，快进到 `side`，`dir/b.txt` 与 `s.txt` 都在。

根因线索（只读定位）：`src/cli/merge.rs` 的 `fast_forward()` 把**已展平**的路径列表经 `flat_tree()`
塞回一个 `Tree`（条目名里带 `/`），交给 `worktree::materialize_tree` 后被 `validate_name` 正确拒绝。
T10（opencode）自己的 `cli_fast_forward_matches_git` 只用了扁平路径，所以没覆盖到。

## 1. 目标（签名不许改）

- `src/cli/merge.rs`：修复快进路径（不要绕过 `validate_name`，也不要把 `/` 当合法条目名）。
  推荐做法：快进时**直接用目标提交的 tree**（经 `Odb` 读出并解码）交给 `materialize_tree`，
  或在内部构造正确的嵌套 `Tree`。
- 行为必须与真实 git 一致：快进时移动分支 ref、更新 index、把工作区物化成目标 tree 的内容
  （含新增/删除/修改、子目录、可执行位、symlink）。

## 2. 写作用域（白名单，只有这两个文件）

- `src/cli/merge.rs`
- `tests/verify_merge_ff.rs`（**新建**，可选）

**不许改**：`src/merge/**`、`src/worktree/**`、`src/refs/**`、`src/odb/**`、`src/transport/**`、
`src/cli/mod.rs`、其它 `src/cli/*.rs`、`tests/fsck_gc.rs`（T15 的文件，里面有钉住本缺陷的
`#[ignore]` 用例，**不要动它**）、`Cargo.toml`、`Cargo.lock`、`.orch/**`。需要改 → 报 `blocked`。

## 3. 验收（必须能独立复跑）

```bash
cd /home/user/Projects/mini-git
cargo test --offline --test verify_merge_ff      # 若你新建了测试
cargo test --offline --test fsck_gc -- --ignored e2e_fast_forward_merge_in_a_repository_with_subdirectories
cargo test --offline                             # 见下方「共享 checkout」说明
cargo clippy --offline --all-targets             # 你的文件 0 warning
scripts/check-freeze.sh                          # drift 0
```

**必做对拍**（平行仓库：同一初始状态，一份跑 mg 一份跑真实 git，逐字段比较
`git log --oneline`、`git status --porcelain`、`git ls-files --stage`、工作区文件树含 mode/symlink）：
1. 上面那个嵌套路径快进（核心）；
2. 快进**删除**子目录里的文件；
3. 快进**新增**一个含多层子目录的路径；
4. 非快进合并回归（T10 的 `cli_merge_*` 用例与你自己造一个真冲突场景）；
5. `mg merge` 在子目录文件冲突时仍与 git 一致（别把 D1 修成新回归）。

## 4. 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：命令 + 真实结论（每条对拍的字段是否逐字节一致、`#[ignore]` 用例现在是否通过）；
  已知限制/未覆盖项单列。

## 5. 非目标

- 不做 rename 检测、不做 `--no-ff`/`--squash` 之外的策略改动、不做 rebase/cherry-pick。
- 不改 `merge::three_way` 的引擎语义（它已被 W3/V10 验证过）。

## 附：前提可被质疑（W3 教训）

任务书里的任何「真实 git 会 X」都只是提示。动手前先自己复跑一遍关键前提；
若与实测不符，**先报冲突（附原始命令与输出），再按实测的真值实现**。

## 附：测试不得「skip 即通过」

任何用例不允许在环境缺失时 `return` 成通过；要么硬失败，要么显式 `#[ignore]`。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律

- 不触发任何额外交互；原生审批只按「单条」处理（选 1，不选 2）。
- 不 export `GIT_*`；用 `git -c key=value` / `env VAR=... git ...`。
- **共享 checkout**：本 wave 另有 T13（codex，正在改 `src/transport/**` 与 `src/cli/{clone,fetch,push,pull}.rs`）
  与 V15（opencode 验证者）在跑。`cargo test --offline` 里属于它们的红/编译错误注明
  「不可归因于本任务」并等 30 秒重试，**不要去改别人的文件**。
- 临时仓库放 `/tmp/<你的名字>/`；变异/临时构建用独立 `CARGO_TARGET_DIR`。
