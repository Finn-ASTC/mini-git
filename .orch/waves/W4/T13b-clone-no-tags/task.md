# T13b —— 修复「远端没有任何 tag 时 `mg clone` 失败」（T15 发现的 D2）

你是 T13 的**同一个作者**（kind: codex），本轮是返工轮。你上一轮交付了 `file://` 传输与
clone/fetch/push/pull；peer 作者 T15 在其 e2e 里发现一个你没覆盖的缺陷，controller 已独立复现。

## 0. 缺陷（controller 复现，git 2.55.0）

```bash
git init -q origin && cd origin && echo a > a.txt && git add . \
  && git -c user.name=t -c user.email=t@e commit -qm one
mg clone file://$PWD/origin /tmp/copy
  → mg: fatal: couldn't find remote ref refs/tags/*      （exit 1，目录未建）
git clone  file://$PWD/origin /tmp/copy2                  → 成功（exit 0）
```
远端**存在** tag 时 mg clone 正常。根因是 clone 无条件请求 `refs/tags/*`，远端没有该 ref 时
advertisement 里查不到就报 fatal，而不是「没有 tag 就没有 tag」。

## 1. 目标

修复 `mg clone`（以及它调用的 fetch 路径）在下列夹具下与真实 git 一致：
1. 远端**无任何 tag**（核心）；
2. 远端**只有轻量 tag** / **只有 annotated tag** / 两者都有；
3. 远端 HEAD 指向的分支名与本地默认分支名不同（例如远端 `main`、本地默认 `master`）——
   真实 git 会建 `refs/remotes/origin/HEAD` 与一个本地分支（判据以你实测的 git 行为为准）；
4. 远端 HEAD detached（广告里没有 `symref=`）——以 git 实测为准；
5. 回归：远端有 tag + 多分支 + 执行位文件时，你上一轮的 `clone_matches_real_git` 仍须通过。

**判据**：`git -C dst status --porcelain` 为空、`git -C dst fsck` 无 error、
`git -C dst show-ref`（排序后）与真实 `git clone` 的产物一致（含 `refs/tags/*` 的有无）。

## 2. 写作用域（白名单）

- `src/cli/clone.rs`
- `src/transport/local.rs`
- `src/cli/fetch.rs`（仅当修复必须在这里）
- `tests/verify_transport.rs` 是 V13 的文件：**不要动**。

**不许改**：`src/transport/{mod,negotiate,http}.rs`、`src/cli/mod.rs`、其它 `src/cli/*.rs`、
`tests/fsck_gc.rs`（T15 的文件，里面有钉住本缺陷的 `#[ignore]` 用例
`clone_of_an_origin_without_tags_succeeds`，**不要动它**）、`Cargo.toml`、`Cargo.lock`、`.orch/**`。

## 3. 验收

```bash
cd /home/user/Projects/mini-git
cargo test --offline transport::
cargo test --offline
cargo test --offline --test fsck_gc -- --ignored clone_of_an_origin_without_tags_succeeds
cargo clippy --offline --all-targets      # 你的文件 0 warning
scripts/check-freeze.sh                   # drift 0
```
外加：把上面 §0 的最小复现（无 tag 远端）的**修复前后**原始输出贴进 result，
以及 §1 的 5 个夹具你实际跑了几个（逐条给命令与关键输出）。

## 4. 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**。这是**新的 round**，不要覆盖上一轮。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：命令 + 真实结论；已知限制/未覆盖项单列。

## 附：前提可被质疑

任务书里的任何「真实 git 会 X」都只是提示（W3/P18 教训）。先自己复跑一遍关键前提；
若与实测不符，先报冲突（附原始命令与输出），再按实测真值实现。

## 附：测试不得「skip 即通过」

任何用例不允许在环境缺失时 `return` 成通过；要么硬失败，要么显式 `#[ignore]`。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律

- 不触发额外交互；原生审批只按「单条」处理（选 1，不选 2）。不 export `GIT_*`。
- **共享 checkout**：T16（已交付，opencode）改了 `src/cli/merge.rs`；V16（omp）可能正在跑测试。
  属于它们的红/编译错误注明「不可归因于本任务」并等 30 秒重试。
- 临时仓库放 `/tmp/<你的名字>/`。
