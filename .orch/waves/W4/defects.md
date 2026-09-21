# Wave W4 缺陷与观察清单

> 范围：W4（T13/T14/T15 三个作者任务 + V13/V14/V15 三个验证者 + T13b/T13c/T14b/T16/T16b 五个返工轮 + V16 跟进验证）。
> **分类**：`产品缺陷`（mini-git 自身行为与真实 git 不一致）/ `流程缺陷`（编排 skill 侧）。
> 跨 wave 的合并视图在 `.orch/SKILL-FINDINGS.md`（编号 P*），本文件用 `W4-*` 编号。

## 产品缺陷（mini-git）

| # | 发现者 | 现象 | 影响 | 处置 |
|---|---|---|---|---|
| W4-D1 | **T15 作者（omp，自查）** | `mg merge` 快进在**含子目录**的仓库里必然失败：`fatal: corrupt tree entry name: "dir/b.txt"`（exit 1）；真实 git exit 0 并快进 | 合并的**主路径**之一完全不可用（只需一个 `sub/dir` 就触发）。T10 自己的对拍只用扁平路径，所以 W3 没覆盖到 | **T16 返工**（opencode）：`fast_forward()` 改用 `odb.read_object(target_tree)` 拿真嵌套 tree；**V16 独立 12 用例 + 2 变异体 PASS** |
| W4-D2 | **T15 作者** | 远端**没有任何 tag** 时 `mg clone` 直接 fatal（`couldn't find remote ref refs/tags/*`）；真实 git 正常克隆 | clone 的常规场景之一不可用（新仓库/未打 tag 的远端）| **T13b 返工**（codex）：通配 refspec 匹配 0 个引用 = 空操作（非通配仍 fatal）；另补 `warning: remote HEAD refers to nonexistent ref`。6 夹具与真实 git 逐字段对拍 |
| W4-D3 | **T15 作者** | 报告 `mg push` 拒绝 bare 远端（误报 `branch is currently checked out`）与新建远端分支（误报 `the remote ref moved`）| **实测为「并行中间态」**，非交付缺陷：T15 观测时 T13 仍在飞行中（共享 checkout）。T13 最终交付里 `checked_out_branch()` 已按 `core.bare` 判定、CAS 已放行 `old = 全 0` 的新建；**V13 复核确认两者都工作**（`tests/fsck_gc.rs` 里钉这两条的 `#[ignore]` 用例实跑通过）| 归因修正（见 S 表 W4-S3）；回归用例由 C-24/C-25 摘掉 `#[ignore]` 转正 |
| W4-D4 | **T15 作者** | `cargo test --offline` 非 0：`src/cli/clone.rs:49` 的单测 `derives_the_directory_name_like_git` 恒失败（`default_dir("file:///")` 的三斜杠被 `trim_end_matches('/')` 削成 `file:`）| 全仓门禁在 T13 落地期间一直红（T15 无法在写作用域内修）| 已消失：T13c 改 `src/cli/clone.rs` 后 `cargo test --offline` → **562 passed / 0 failed**（controller 收口复跑）|
| W4-D5 | **T15 作者** | `cargo clippy --offline --all-targets` 5 条 warning：`src/error.rs:62`（`todo` 死代码）、`src/transport/local.rs:611/463/1280`、`src/transport/negotiate.rs:22` | 质量门非绿（W0 起就承诺 clippy 干净）| 4 条由并行轮（T13/T14/T15）各自修掉；**最后 1 条 `error.rs::todo` 是 CONTROLLER-OWNED**，controller 删除（FREEZE → v0.6）。现全仓 **0 warning** |
| W4-D6 | **T15 作者** | 卫生项：`src/cli/merge.rs::skip_if_not_implemented`（`#[cfg(test)]` 辅助）在 stderr 含 `not implemented yet` 时 `return`，会把断言静默放行 | 与任务书「测试不得 skip 即通过」冲突（W3-D7 同源）；当前全仓已无 `todo()` 调用点，该分支不可达，V16 实测 SKIP 行数 = 0 | **不修**（记为卫生项）。若要收紧：把 `skip_if_not_implemented` 改成硬失败 |
| W4-D7 | **V14（codex 验证者）** | GET `info/refs` 的 `Accept` 头多一层 `git-`：`application/x-git-git-upload-pack-advertisement`；真实 git 2.55.0 发 `Accept: */*` | 低严重度（影响前置代理/缓存的状态码与重写行为）。作者的 9 个自测**完全没覆盖 Accept** —— 是「断言盲区」而非假绿 | **T14b 返工**（opencode）：改为 `*/*`，用原始 TCP 抓包 + 与真实 git 客户端逐字对拍自证 |
| W4-D8 | **V13（omp 验证者）** | 空远端（`git init` 无提交 / 空裸库）时 `mg clone` 报 Err 且不留目录；真实 git **exit 0** + `warning: You appear to have cloned an empty repository.` + 留下只含 `.git` 的克隆 | 与 git 不一致；且**任务书自己把这个错误行为写成了「必须报清晰 Err」的验收项**（V13 用实测推翻） | controller 裁决**跟随真实 git**（C-24）→ **T13c 返工**（codex）：exit 0 + 逐字 warning + 只留 `.git`，10 项测试与 git 逐字段一致 |
| W4-D9 | **V16（omp 验证者）** | `mg merge` 的本地改动守卫是**全仓脏即拒绝**；真实 git 只在「本地改动会被本次合并覆盖」时拒绝（不会被碰的文件上 git 允许快进并保留改动）| mg 比 git **严**（过度拒绝），破坏「快进与 git 一致」的字面验收 | **T16b 返工**（opencode）：全仓守卫 → **按路径**判定（触及路径 index 守卫 + 非快进 index==HEAD 守卫 + 逐路径 ensure_clean）。6 组夹具 + 额外边界全对齐 |
| W4-D10 | **V15（opencode 验证者）** | 已确认分歧：`mg fsck` 只遍历**可达**对象，`git fsck` 还扫不可达的 loose 对象 → 不可达的损坏对象 mg 漏报 | 覆盖缺口（非互操作硬伤） | **不修**：`tests/verify_fsck_gc.rs` 用一条 `#[ignore = "known divergence: …"]` 钉住，写进 README 已知限制 |
| W4-D11 | **V16** | `mg pull` 冲突标记的 theirs 标签是 `refs/remotes/origin/main`，`git pull` 用 oid（`>>>>>>> a9dd111…`）。补充真值：同一仓库 `git merge refs/remotes/origin/main` 的标签**正是** `refs/remotes/origin/main` —— mg 与 `git merge <ref>` 完全一致，差异只源于 `git pull` 合并 `FETCH_HEAD` | 仅影响冲突标记文本；porcelain / index stage / 正文全部一致 | **不修**：记入 README 已知限制（属 `pull` 的标签选择，跨 T13/T10 边界）|
| W4-D12 | **release 构建专项测试（2026-09-20 补测，非任何 agent 轮次发现）** | **`mg` 的 release 构建存在死循环**：`mg diff` 在特定输入上 100% CPU 永久挂起；`mg merge`（非快进/三方）同样挂起。根因：`src/diff/myers.rs::change_compact` 把**带副作用的调用**写进了 `debug_assert!`（`group_previous` / `group_next` / `group_slide_up`，共 8 处，行 906/918/934/935/961/962/970/973）——debug 下它们被求值并推进游标 `go`，release 下整条被编译掉，于是 `while go.end == go.start { … }` 与 `while g.end > best_shift { … }` 变成空循环 | **发布构建不可用**：①永久挂起（无错误、无输出）；②即使不挂起，输出也与 git 不一致（压缩块没有滑动）。**debug 构建完全正常且与 git 逐字节一致** | **未修**（用户裁决「暂时不修」）。修法：把这 8 处拆成「先调用并把返回值绑到变量，再 `debug_assert!(变量)`」，或直接去掉断言保留调用 |

**W4-D1 的最小复现**（T15 报告原文，controller 已复跑）：

```bash
cd /tmp && rm -rf w4d1 && mkdir w4d1 && cd w4d1
MG=/home/user/Projects/mini-git/target/debug/mg
$MG init .
mkdir dir && echo beta > dir/b.txt && $MG add . && $MG commit -m c1
$MG switch -c side main && echo s > s.txt && $MG add s.txt && $MG commit -m c2
$MG switch main
$MG merge side        # 修复前：fatal: corrupt tree entry name: "dir/b.txt" (exit 1)
                      # 修复后：Fast-forward (exit 0)；dir/b.txt=beta、s.txt=s
```

**W4-D2 的最小复现**：

```bash
cd /tmp && rm -rf w4d2 && mkdir w4d2 && cd w4d2
git init -q origin && cd origin && echo a > a.txt && git add . && git -c user.name=t -c user.email=t@e commit -qm one
$MG clone file://$PWD/origin /tmp/w4d2/copy      # 修复前：fatal: couldn't find remote ref refs/tags/* (exit 1)
git clone  file://$PWD/origin /tmp/w4d2/copy2    # 真实 git：成功
```

**W4-D9 的夹具对照**（V16 的 F1，T16b 修复后 controller 复跑）：

| 夹具 | 真实 git | mg（T16b 后） |
|---|---|---|
| 本地改动在**不会被覆盖**的路径上 + 快进 | exit 0，保留改动 + 物化目标文件 | 一致 |
| 本地改动在**会被覆盖**的路径上 + 快进 | exit 1，工作区/index/refs 零改动 | 一致 |
| 非快进干净合并 + 无关本地改动 | exit 0（实测，任务书是提示） | 一致 |

## 流程缺陷（编排 skill 侧）

| # | 现象 | 证据 | 对应 P 编号 |
|---|---|---|---|
| W4-S1 | **门禁测试所有权债务连续第二个 wave 成为返工主因**：本 wave 出现 3 例「别人的测试文件钉住旧行为」→ child 报 blocked（V13 的 Q2、T14b 的 Q1、T13c 的 Q1）。**全部没有越界改** | `.orch/rounds/W4/{agent-orchestrator-bc1br750,agent-orchestrator-gvvgao3_,agent-orchestrator-t2z2r0_i}/result.json` §5；`ORCHESTRATION.md` C-24 / C-27 / C-28 | **P17** 再现 |
| W4-S2 | **过期的 `#[ignore]` 不会自己失效**：controller 收口复跑 `cargo test --offline -- --ignored` 才发现 4 处恒绿；其中 `tests/verify_diff.rs::cli_space_in_path_tab_padding` 是 **W2/T6b 修好后遗留**，跨了两个 wave 一直「静默关闭」 | `ORCHESTRATION.md` C-25；`SKILL-FINDINGS.md` **P20** | **P20**（本轮新增） |
| W4-S3 | **并发 wave 里「谁改了哪个文件」协议给不出答案**：T15（21:54 完成）报的 D3 在 V13 复核（22:21+）时已好 —— 它观测到的是 **T13 尚在飞行中的中间态**。协议没有「制品版本/哈希」概念，作者无法知道自己 build 的是哪一版 | T15 result §4-D3 vs V13 result §5-Q2；`SKILL-FINDINGS.md` **P13** 延伸 → **P21** | **P21**（本轮新增） |
| W4-S4 | **验证者与作者共享 checkout，验证期间制品仍在变动**：V13 跑门禁时 `tests/verify_http_http.rs`（peer V14 的文件）正在被改写，导致 `cargo test --offline` 出现 6 failed，V13 必须手工逐个 target 归因（它做到了，并明确「不可归因于本任务」）| V13 result §1 的表格与脚注 | **P13** 再现 |
| W4-S5 | **任务书自己指定的「真值」又错 2 处**：① T13/V13 把「空远端必须报 Err」写成验收项（真实 git 是 exit 0 + warning）；② T16 的「快进时工作区有本地改动→双方都拒绝」前提过宽。两处都由验证者用实测推翻 | V13 result §4；V16 result §D | **P10 / P11** 再现 |
| W4-S6 | **任务书里的路径会过期**：`T13-transport/baseline.txt` 其实不存在（真实基线是 wave 级的 `.orch/waves/W4/baseline.txt`），V13 自己找到替代品 | V13 result §4.3 | **P12** 同类 |
| W4-S7 | **`purpose` 词表仍缺 verify/rework**：本 wave 7 个 manifest 再次全部填 `task`，验证轮/返工轮的成本只能在 summary 里手工分列 | `.orch/artifacts/usage-manifests/*.json`；`ORCHESTRATION.md` C-25 的记账原则 | **P19** 再现 |
| W4-S8 | **跨 kind 成本对比仍不成立**：opencode 无原生适配器，5/12 轮（T14/T16/T16b/V15 等）无计数 | `usage.py summary --request …` 对 5 个 round 返回 `null` | **P15** 再现 |
| W4-S9 | **变异测试的编译产物污染会产生假结论**：V16 的两个变异体最初共用同一个 `CARGO_TARGET_DIR`，mutB 复用 mutA 的产物 → 症状完全相同。改成每变异体独立 target 后才正确区分 | V16 result §C 的「方法论警告」 | **P18** 同源（结论必须自证夹具与制品）|
| W4-S10 | **hermes 在本 wave 缺席**：W1–W3 都有 hermes，W4 只有 codex/omp/opencode。这是 controller 的排期选择（hermes 完成度最高但 wall 最慢），不是故障；但「4 kind 异构」的对比基线在这一 wave 退化为 3 kind | W4 12 个 round 的 agent kind 分布 | 新观察（不予编号，需在横向对比时注明）|
| W4-S11 | **归档证据可被测试改写**：`tests/verify_pack.rs:566` 把 V9 的原始输出写进 `.orch/waves/W3/T9-pack/verify-scratch/`，于是**任何一次 `cargo test` 都会重写已归档的历史证据**（收口复跑时 10 个 `v9-*.txt` 的 mtime 全部变成「现在」）。内容是确定性的、未失真，但「证据不可变」这一前提被打破了 | `ls -la .orch/waves/W3/T9-pack/verify-scratch/`（mtime 全为收口时刻）；`grep -rn 'verify-scratch' tests/`（唯一命中）| 新观察（与 P13/P21 同源：制品与证据的版本归属没有定义）|
| W4-S12 | **所有门禁都只在 debug profile 下跑过**：W1–W4 的 40 轮、所有验证任务书的门禁清单都写的是 `cargo test --offline`（默认 debug），**没有任何一轮跑过 `cargo test --release`**，于是「`debug_assert!` 的副作用在 release 下消失」这类缺陷在 4 个 wave、17 个验证轮里**完全不可见** —— 而它同时是挂起 + 输出错误 | 收口补测：`cargo test --release --offline` | **新 P 编号：P23** |

## 处置记录（controller 侧，本 wave 收口）

| 动作 | 结果 |
|---|---|
| C-25 清理 4 处过期 `#[ignore]` | `fsck_gc` → 25 passed / **0 ignored**；`verify_diff` → 17 passed / **0 ignored**；全仓仅剩 1 个 `#[ignore]`（W4-D10 的已知分歧）|
| 删除 `src/error.rs` 的 W0 桩函数 `todo()` | FREEZE → **v0.6**；`cargo clippy --offline --all-targets` 全仓 **0 warning** |
| 翻转 `tests/verify_http_http.rs` 两处 characterization 断言 + 补 `LC_ALL=C` | 18 passed / 0 failed（T14b 的 blocked 解除）|
| 翻转 `tests/verify_transport.rs` 的空远端 clone 断言（改为与真实 git 同向）| 10 passed / 0 failed（T13c 的 blocked 解除）|
| 全仓 `cargo fmt --all`（C-29）| 47 个文件被格式化（唯一冻结文件 `src/odb/pack/mod.rs`，纯空白）；FREEZE → **v0.7**；`cargo fmt --all -- --check` exit 0 |
| 全仓最终门禁 | `cargo test --offline --no-fail-fast` → **562 passed / 0 failed / 1 ignored**，EXIT=0；`clippy --offline --all-targets` **0 warning**；`cargo fmt --all -- --check` exit 0；`check-freeze.sh` → `checked 21 file(s), drift 0` |
| **release 专项测试（2026-09-20，用户要求）** | `cargo test --release --offline` → **lib 与 `verify_diff` 两个 target 卡死（exit 124）**，其余 20 个 target **272 passed / 0 failed / 1 ignored**；release 二进制在 200 例随机语料上：**3 例死循环、12 例输出与 debug/git 不同**（case 35 实证 `debug == git`、`release != git`）。根因与修法见 W4-D12；**未修** |
| `cargo test --offline -- --ignored` 对账 | 全仓只剩 **1 个** `#[ignore]`：`tests/verify_fsck_gc.rs::fsck_detects_corruption_in_unreachable_objects`，实跑 **FAILED**（= W4-D10 的已确认分歧，符合预期）|
