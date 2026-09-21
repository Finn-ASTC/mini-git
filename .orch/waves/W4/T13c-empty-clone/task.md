# T13c —— 空远端仓库的 `mg clone` 对齐真实 git（V13 的裁决问题 Q1）

你是 T13/T13b 的**同一个作者**（kind: codex），本轮是返工轮。controller 已裁决：**跟随真实 git**。

## 0. 问题（V13 报告，controller 复核）

当前 `mg clone file://<空仓库>` → exit 1 `remote repository '…' is empty (no refs advertised)`，不留目录。
真实 git 2.55.0 对同一夹具（`git init --bare -b main` 的空裸库 / 空的非裸库）是：

```
warning: You appear to have cloned an empty repository.
exit 0，留下一个只含 .git 的克隆（无检出文件）
```

## 1. 目标

让 `mg clone` 在「advertisement 为空仓库」时与真实 git 一致：
1. exit 0；
2. 打印与真实 git **逐字相同**的 warning 行（`warning: You appear to have cloned an empty repository.`）；
3. 建立目标仓库（含 remote 配置与 HEAD 指向远端 HEAD 的符号引用，判据以 `git -C dst show-ref`、
   `git -C dst symbolic-ref HEAD`、`git -C dst status --porcelain` 与真实 `git clone` 逐字段比对为准）；
4. 跳过 fetch 与 checkout；不产生工作区文件；
5. **回归**：非空远端、无 tag 远端、只有轻量/annotated tag 的远端（T13b 的 6 个夹具）全部保持通过。

`src/cli/fetch.rs` 对空远端的处理也请一并复核（真实 `git fetch` 在空远端上 exit 0 且不写引用）。

## 2. 写作用域（白名单）

- `src/cli/clone.rs`
- `src/transport/local.rs`
- `src/cli/fetch.rs`（仅在必要时）

**不许改**：`src/transport/{mod,negotiate,http}.rs`、`src/cli/mod.rs`、其它 `src/cli/*.rs`、
`tests/verify_transport.rs`（V13 的文件）、其它 `tests/**`、`Cargo.toml`、`Cargo.lock`、`.orch/**`。

## 3. 验收

```bash
cargo test --offline transport::
cargo test --offline
cargo test --offline --test verify_transport     # V13 的，必须仍然全绿
cargo clippy --offline --all-targets             # 你的文件 0 warning
scripts/check-freeze.sh
```
外加：把「空裸库 / 空非裸库」两个夹具的 **mg vs 真实 git** 逐字段对拍
（exit code、stderr 文本、目标目录内容、`show-ref`、`symbolic-ref HEAD`、`status --porcelain`）
原始输出贴进 result。

## 4. 上报

- result 路径：由 prompt.txt 给出（**新 round**）。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。

## 5. 非目标

- 不改 push/pull 语义；不新增依赖。

## 附：前提可被质疑 / 测试不得 skip 即通过 / 交互纪律

- 如果实测真实 git 的行为与本任务书写的不一致 → 先报冲突（附原始输出），按实测真值实现。
- 环境缺失时不许 `return` 成通过；要么硬失败，要么 `#[ignore]`。
- 原生审批只按「单条」处理（选 1，不选 2）；不 export `GIT_*`；临时仓库放 `/tmp/<你的名字>/`。
