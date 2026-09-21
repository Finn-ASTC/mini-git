# mini-git

一个**与真实 git 二进制互操作**的 git 子集实现（CLI 名为 `mg`），
同时作为**异构 agent 编排 skill 的测试床**。

两件事说清楚：

* **工程目标**：`mg` 与 `git` 读写同一个 `.git` 目录，用真实 `git` 当不可作弊的验收 oracle。
* **测试床目标**：让 omp / hermes / opencode / codex 四种 agent 在互斥写作用域下并行协作，
  并采集「编排 skill 是否真的被正确执行」的可复现证据。

> 现状：**v1 完成**。W0（骨架）→ W1（对象/索引/引用/工作区）→ W2（odb/diff/merge-base/pkt-line）
> → W3（pack/三方合并/物化/plumbing）→ W4（传输层 + `fsck`/`gc` 收口）全部收口，
> 21 个子命令全部实现；全仓 **562 passed / 0 failed**（dev 与 release 两个 profile 均如此）、
> 仅 1 个 `#[ignore]`（已确认分歧）、`clippy` 0 warning、`fmt` 干净。

## 文档

| 文件 | 内容 |
|---|---|
| `PLAN.md` | 技术规划：数据模型、CLI 表面、里程碑、测试策略、风险 |
| `ORCHESTRATION.md` | 多 agent 架构：写作用域、依赖 DAG、Wave 计划、评测指标、决策记录（C-1 ~ C-28）|
| `.orch/SKILL-FINDINGS.md` | **测试床的核心产出**：P1–P24 编排 skill 缺陷清单（含最小复现与修法）|
| `.orch/waves/W{1..4}/summary.md` | 每个 wave 的复盘（含指标表、token 账本、验证者发现）|
| `.orch/waves/W{1..4}/defects.md` | 每个 wave 的产品缺陷与流程缺陷 |
| `.orch/FREEZE-v0.md` | W0 接口冻结清单（哈希基线，代替 git tag；现为 v0.6）|
| `.orch/README.md` | 编排产物布局与协议要点 |
| `.orch/artifacts/` | **编排过程的原始归档**：controller 自建工具、codex 审批日志、token 清单、验证者对拍 harness 与变异体（原先散在 `/tmp`，重启即丢）|

## 构建与验证

> ✅ **P24 已修（2026-09-21，W7）**：原先 `--release` 构建的 `mg` 在 `mg diff` / `mg merge` 上会死循环
> （根因：`src/diff/myers.rs::change_compact` 有 8 处 `debug_assert!` 包住了带副作用的调用，
> release 下这些状态推进被整条编译掉）。现已改成「先调用（副作用一定发生）再断言返回值」，
> release / debug / 真实 git 三者在 200 例随机语料上逐字节一致、零挂起。
> 证据：`.orch/waves/W7/summary.md`、`.orch/SKILL-FINDINGS.md` 的 **P23 / P24**。

```bash
cargo build --offline            # 依赖已在本机 cargo 缓存里，可离线构建
cargo test  --offline            # dev：562 passed / 0 failed / 1 ignored（唯一的 ignore 是已确认分歧，见下）
cargo test  --release --offline  # 交付门禁：同样 562 passed / 0 failed / 1 ignored
# ↑ 两条都要跑：P23 的教训就是「只在 debug profile 下跑门禁」放过了 P24 这种 release 专属缺陷。
cargo clippy --offline --all-targets   # 必须 0 warning
cargo fmt --all --check                # 必须无 diff
scripts/check-freeze.sh                # 接口漂移检测（0 = 冻结基线完好）
```

> 门禁与宿主 locale 无关：所有对拍真实 git 的测试都把 git 的 locale 钉成 `LC_ALL=C`。
> 否则 `LC_MESSAGES=zh_CN.UTF-8` 这类环境下 git 会打印中文，而逐字节比较 git 英文 stderr
> 的用例（空远端 clone 的 warning 等）会假红。

> 跑 HTTP 传输测试（`tests/verify_http*.rs`）需要能 bind `127.0.0.1`；
> 在受限沙箱里这些用例会以 `PermissionDenied` 失败，需要非沙箱环境运行。

## 用法

```bash
cargo run --offline -- init /tmp/demo          # 或直接跑 target/debug/mg
cd /tmp/demo && git status                     # 真实 git 能直接用这个仓库
printf 'hello world\n' | mg hash-object --stdin
# => 3b18e512dba79e4c8300dd08aeb37f8e728b8dad（与 git 逐字节相同）

mg clone file:///path/to/origin /tmp/copy && cd /tmp/copy && mg log --oneline
mg fsck && mg gc
```

## `mg` ↔ `git` 命令对照

| `mg` | `git` | 状态 |
|---|---|---|
| `mg init` | `git init` | ✅ |
| `mg hash-object [--stdin] [-w] [-t]` | 同 | ✅ |
| `mg cat-file (-t\|-s\|-p\|-e)` | 同 | ✅ |
| `mg add` / `mg rm [--cached]` | 同 | ✅ |
| `mg status [--porcelain]` | 同 | ✅ |
| `mg commit [-m] [-a] [--amend]` | 同 | ✅ |
| `mg log [--oneline] [-n]` | 同 | ✅ |
| `mg diff [--staged] [-U]` | 同 | ✅ |
| `mg branch` / `mg switch` / `mg checkout` / `mg reset [--soft\|--mixed\|--hard]` | 同 | ✅ |
| `mg merge` | 同 | ✅ |
| `mg tag [-a] [-m]` | 同 | ✅ |
| `mg clone` / `fetch` / `pull` / `push` | 同 | ✅（`file://` 与 `http://`）|
| `mg fsck [--full]` / `mg gc` | 同 | ✅ |

## v1 明确不做

`rebase`、`cherry-pick`、`submodule`（gitlink 只读并报 `Unsupported`）、`worktree`、交互式操作、
`core.autocrlf` 转换、重命名检测、index v3/v4、协议 v2、shallow/partial clone、
`https://`（不引 TLS 依赖）、`git://`、带主机的 `file://`、SHA-256 仓库、
ref 删除 / `--mirror` / `--tags` / `--all` / push-cert、thin pack 与 `REF_DELTA` 的**写**侧。

## v1 已知限制（实测确认，非缺陷）

| 限制 | 详情 |
|---|---|
| `merge.conflictstyle=zdiff3` | 退化成 diff3（两侧公共行留在 marker 内）——W3/V10 实测，见 C-22 |
| `mg fsck` 的扫描范围 | 只遍历**可达**对象；`git fsck` 还扫不可达的 loose 对象 → 不可达的损坏对象 mg 漏报（用 `#[ignore]` 钉住）|
| `mg pull` 的冲突标记 | theirs 标签用 `refs/remotes/origin/main`，`git pull` 用 oid；与 `git merge <ref>` 完全一致 |
| `mg merge` 的 commit oid | 用自身身份与当前时间，不可与 git 逐字节相同（对拍用 tree/parents/porcelain）|
| `receive.denyCurrentBranch=updateInstead` | 仍拒绝（真实 git 会更新远端工作区）|
| `core.quotePath=false` | 不生效：mg 不读任何 git 配置，路径一律按默认的 `true` 渲染引用（`status` / `diff` / `cat-file -p` 同一套规则，见 `cli::diff::quote_path`）|
| `mg fetch <url>`（未配置远端） | 会写 `refs/remotes/origin/*`，真实 git 只写 `FETCH_HEAD` |
| merge 输出 | 比真实 git 简（不打印 `Merge made by the 'ort' strategy.`）|
| pack 写出 | 只写**无 delta** 的 pack（`mg gc`）；读取侧支持 OFS/REF delta |
| push | 多 ref 全有或全无 |

## 文件所有权（并行开发的前提）

* **CONTROLLER-OWNED**：`Cargo.toml`、`src/lib.rs`、`src/main.rs`、
  各 `src/*/mod.rs` 的 `pub` 声明区、`tests/interop/**`、`.orch/**`、`scripts/**`。
* **MODULE-OWNED**：每个子模块的实现文件属于该模块的负责 agent（见 `ORCHESTRATION.md` §4）。
* 需要新依赖或改公共签名 → 走一次 controller round（场景 S9）。

> 本工作区在开发期**没有 git 历史**（沙箱里 `.git` 空且只读），冻结基线用
> `.orch/FREEZE-v0.md` + `scripts/check-freeze.sh` 表达；发布时才 `git init`
> （首个提交 4521 files，远端 `Finn-ASTC/mini-git`），冻结基线与 git tag 并存、各管一段：
> `check-freeze.sh` 管的是「公共接口有没有被越权改动」，比 tag 更细。
