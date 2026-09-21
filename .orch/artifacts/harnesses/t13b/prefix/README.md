# mini-git

一个**与真实 git 二进制互操作**的 git 子集实现（CLI 名为 `mg`），
同时作为**异构 agent 编排 skill 的测试床**。

两件事说清楚：

* **工程目标**：`mg` 与 `git` 读写同一个 `.git` 目录，用真实 `git` 当不可作弊的验收 oracle。
* **测试床目标**：让 omp / hermes / opencode / codex 四种 agent 在互斥写作用域下并行协作，
  并采集「编排 skill 是否真的被正确执行」的可复现证据。

> 现状：**W0（骨架 + 测试床）已完成**，L1 及之后的模块是冻结签名的桩。
> 当前应实现 `todo("... (T<n>)")` 标记的位置。

## 文档

| 文件 | 内容 |
|---|---|
| `PLAN.md` | 技术规划：数据模型、CLI 表面、里程碑、测试策略、风险 |
| `ORCHESTRATION.md` | 多 agent 架构：写作用域、依赖 DAG、Wave 计划、评测指标、场景注入 |
| `.orch/FREEZE-v0.md` | W0 接口冻结清单（哈希基线，代替 git tag）|
| `.orch/README.md` | 编排产物布局与协议要点 |

## 构建与验证

```bash
cargo build --offline          # 依赖已在本机 cargo 缓存里，可离线构建
cargo test  --offline          # 15 个单元测试 + 7 个与真实 git 的差分冒烟测试
cargo clippy --offline --all-targets   # 必须 0 warning
cargo fmt --all
scripts/check-freeze.sh        # 接口漂移检测（0 = 冻结基线完好）
```

## 用法

```bash
cargo run --offline -- init /tmp/demo          # 或直接跑 target/debug/mg
cd /tmp/demo && git status                     # 真实 git 能直接用这个仓库
printf 'hello world\n' | mg hash-object --stdin
# => 3b18e512dba79e4c8300dd08aeb37f8e728b8dad（与 git 逐字节相同）
```

## `mg` ↔ `git` 命令对照

| `mg` | `git` | 状态 |
|---|---|---|
| `mg init` | `git init` | ✅ W0 |
| `mg hash-object [--stdin] [-w] [-t]` | 同 | ✅ 不带 `-w`；`-w` 随 T5 |
| `mg cat-file (-t\|-s\|-p\|-e)` | 同 | T5 |
| `mg add` / `mg rm` | 同 | T12 |
| `mg status [--porcelain[=v1]]` | 同 | T4 |
| `mg commit [-m] [-a] [--amend]` | 同 | T12 |
| `mg log [--oneline] [-n]` | 同 | T12 |
| `mg diff [--staged] [-U]` | 同 | T6 |
| `mg branch` / `mg switch -c` / `mg checkout` / `mg reset` | 同 | T11 |
| `mg merge` | 同 | T10 |
| `mg tag [-a] [-m]` | 同 | T12 |
| `mg clone` / `fetch` / `pull` / `push` | 同 | T13（`file://`）/ T14（`http://`）|
| `mg fsck` / `mg gc` | 同 | T15 |

## v1 明确不做

`rebase`、`submodule`、`worktree`、交互式操作、`core.autocrlf` 转换、重命名检测、
index v3/v4、协议 v2、shallow/partial clone、`https://`（不引 TLS 依赖）、SHA-256 仓库。

## 文件所有权（并行开发的前提）

* **CONTROLLER-OWNED**：`Cargo.toml`、`src/lib.rs`、`src/main.rs`、
  各 `src/*/mod.rs` 的 `pub` 声明区、`tests/interop/**`、`.orch/**`、`scripts/**`。
* **MODULE-OWNED**：每个子模块的实现文件属于该模块的负责 agent（见 `ORCHESTRATION.md` §4）。
* 需要新依赖或改公共签名 → 走一次 controller round（场景 S9）。

> 本工作区的 `.git` 是空且只读的（沙箱限制），因此当前无法 `git commit` / `git tag`；
> 冻结基线用 `.orch/FREEZE-v0.md` + `scripts/check-freeze.sh` 表达。
