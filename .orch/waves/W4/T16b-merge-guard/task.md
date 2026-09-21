# T16b —— 让 `mg merge` 的「本地改动」守卫与真实 git 一致（V16 发现的 F1）

你是 T16 的**同一个作者**（kind: opencode），本轮是返工轮。你上一轮修好了嵌套路径快进（D1）。
独立验证者 V16（omp）在复验时用**边界探针**发现：`mg merge` 对本地改动的拒绝比真实 git **更严**。

## 0. 缺陷（V16 报告，controller 待复核）

夹具：仓库里有一个**与本次合并无关**的本地改动文件（`only.txt`，不被任何待更新路径触及），
远端分支要快进引入一个新文件 `s.txt`：

```
git： `Updating bbb49b9..f5cae5b / Fast-forward` exit=0 —— 保留 only.txt 的本地改动，并物化 s.txt
mg ： `mg: fatal: refusing to overwrite local changes: you have local changes; commit or stash them before merging` exit=1，工作区零改动
```
来源：`src/cli/merge.rs::merge_into` 第 64-70 行的**全仓 worktree.status 前置守卫**（T10 的代码，T16 未改）。

真实 git 的规则是**按路径**的：只有当**本次合并/快进会更新或删除的那个路径**存在本地改动时才拒绝；
与合并无关的本地改动必须被保留并放行。

## 1. 目标

把 `mg merge` 的守卫从「全仓脏就拒绝」改成与真实 git 一致的**按路径**判定，覆盖：
1. **快进**：无关路径有本地改动 → 放行，保留该改动，物化目标 tree（只更新本次会变的路径）；
2. **快进**：**会被更新/删除的路径**有本地改动 → 拒绝、工作区/index/refs 零改动（现状已对，不许放松）；
3. **非快进干净合并**：无关路径有本地改动 → 以真实 git 实测为准（先自己测，别照抄本任务书）；
4. **非快进**：会被合并写到的路径有本地改动 → 拒绝、零改动；
5. **未跟踪文件**会被目标新增覆盖 → 拒绝（与 git 一致）；
6. 冲突场景（UU/AA/DU/UD 各一）在「无关路径有本地改动」下与 git 行为一致。

**判据**：平行仓库对拍（同初始态 cp -a 两份，一份 git、一份 mg），比较
`HEAD oid`、`status --porcelain`、`ls-files --stage`、工作区清单（内容 + 权限位 + symlink 目标）、
以及**被保留的本地改动是否逐字节原样**。

## 2. 写作用域（白名单）

- `src/cli/merge.rs`
- `tests/verify_merge_ff.rs`（**你自己上一轮创建的文件**，可以扩展；不要动别的测试文件）

**不许改**：`src/merge/**`、`src/worktree/**`、`src/refs/**`、`src/index.rs` 或 `src/index/**`、
`tests/verify_merge_ff2.rs`（V16 的文件）、`tests/fsck_gc.rs`（T15 的文件）、其它 `tests/**`、
`Cargo.toml`、`Cargo.lock`、`.orch/**`。需要改 → 报 `blocked`。

## 3. 验收

```bash
cd /home/user/Projects/mini-git
cargo test --offline merge
cargo test --offline --test verify_merge_ff           # 你的
cargo test --offline --test verify_merge_ff2          # V16 的（只读运行，必须仍然全绿）
cargo test --offline
cargo clippy --offline --all-targets                  # 你的文件 0 warning
scripts/check-freeze.sh                               # drift 0
```

**注意**：`tests/verify_merge_ff2.rs::` 里那条「快进 + 本地改动会被覆盖（非 force）→ 双方都拒绝」的用例
必须**继续通过**（它说的本地改动落在会被更新的路径上）。

## 4. 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**。这是**新 round**，不要覆盖上一轮。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：命令 + 真实结论（每条夹具的双方行为、被保留的本地改动是否逐字节原样）；
  已知限制/未覆盖项单列。

## 5. 非目标

- 不做 rename 检测、不做 stash、不做 `--abort`/`--continue` 的新语义。
- 不改 `merge::three_way` 引擎（W3/V10 已验证）。
- 不改 index 的写入语义（只改「什么时候允许动手」的判定）。

## 附：前提可被质疑

任务书里的「真实 git 会 X」都只是提示（W3/P18）。先自己复跑夹具；若与实测不符，
先报冲突（附原始命令与输出），按实测真值实现。

## 附：测试不得「skip 即通过」

环境缺失时 `return` 成通过的写法一律禁止；要么硬失败，要么显式 `#[ignore]`。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律

- 不触发额外交互；原生审批只按「单条」处理（选 1，不选 2）。不 export `GIT_*`。
- **共享 checkout**：V13（omp）与 V14（codex）正在并行验证 `src/transport/**` 与 `src/cli/{clone,fetch,push,pull}.rs`。
  属于它们的红/编译错误注明「不可归因于本任务」并等 30 秒重试。
- 临时仓库放 `/tmp/<你的名字>/`；变异测试用独立 `CARGO_TARGET_DIR`。
