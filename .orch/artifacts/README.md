# `.orch/artifacts/` —— 编排过程的原始产物归档

本目录是 **W4 收口时对 `/tmp` 的抢救性归档**：整套异构 agent 编排（W1–W4）用到的自建工具、
审批日志、原生 token 清单、开工前的 skill 自检产物、验证者的对拍 harness 与变异体，
原本都散落在 `/tmp` 里（**重启即丢**），现全部原样复制到这里长期保存。

> 来源、原始 mtime 与归档后的哈希：见 `PROVENANCE.md`（单文件）与 `harnesses/MANIFEST.json`（目录级）。
> 归档只复制、不修改；`/tmp` 里的原件仍然在（`/tmp` 清空后本目录是唯一副本）。

## 目录结构

```
.orch/artifacts/
├── README.md              # 本文件
├── PROVENANCE.md          # 每个文件的原始路径 / mtime / sha256
├── tools/                 # ★ controller 自建工具（P1/P5 的绕过方案）
├── logs/                  # codex 原生审批的逐条记录（P5 的原始证据）
├── usage-manifests/       # 19 份原生 token 导入清单（W2/W3/W4）
├── preflight/             # 开工前的 skill 自检产物（17:1x–19:3x）
├── harnesses/             # 验证者的对拍脚本 / 夹具 / 变异体
├── mutants/               # W3/V11b 的变异体（symlink 修复被回退的源码副本）
├── notes/                 # 收口时从 result.json 抽出的片段（写文档用的中间产物）
└── release-p24/           # ★ release 专项测试的现场（P23/P24 的原始证据）
```

## 1. `tools/` —— controller 自建工具（**P1 的绕过方案**）

`agent-orchestrator` 的 `record` / `watch` / `cleanup-plan` 在用户既存会话（insider 模式）里
因为 pane ID 校验（只认 `w[0-9]+:p[0-9]+`，而 herdr 在既存会话里给的是 `w15:p1` 这类字母型 ID）
**整条不可用**（P1）。controller 改用了自己写的监督循环，这些就是要保留的实现：

| 文件 | 作用 | 关联 |
|---|---|---|
| `orch-supervise.sh` | `status` / `tail <pane> [n]` / `watch`；读 `orch-targets.txt` 轮询结果文件 + 抓 pane 输出，**替代 `watch.py`** | P1、P6 |
| `orch-targets.txt` | 目标表：`agent\|pane\|result_path` | P1 |
| `orch-launch.sh` | 一键起一个可见 space 并投递任务（`workspace create --no-focus` + `agent start`） | C-6、P4 |
| `orch-reuse.sh` | **在既存 agent 会话里续轮**（返工/复验轮用，省冷启动） | C-24、P19 |
| `orch-submit.py` | 与 `protocol.py prepare` 配套的提交器（`PANE=` / `WS=` 环境变量） | P2 |
| `orch-record.py` | 按 `resources.json` 的 schema 手工落盘资源记录（`record` 不可用时的替代） | P1、C-7 |
| `orch-autogrant.py` | codex 审批半自动批准：**只对命中白名单（写 `/tmp`、只读仓库）的命令按 1**，危险模式（`rm -rf /`、`sudo`、`git reset/checkout/clean`、写 `src/`）停机 | **P5**、C-17 |

> ⚠️ `orch-autogrant.py` 必须用 `setsid nohup … < /dev/null &` 启动，否则会随调用它的会话被杀（实测过一次）。

## 2. `logs/orch-approvals.log` —— **P5 的原始证据**

38 行，每条是 `时间 pane APPROVED <命令>`。codex 对复合 bash **逐条**弹原生审批，是整套编排里
唯一的吞吐瓶颈（W1 单轮 4 次、W2 的 V6 一分钟内多次、W3 的 V11 一轮 ≥ 4 次）。
这份日志是「审批风暴有多密集」的逐条时间戳记录。

## 3. `usage-manifests/` —— 原生 token 清单（19 份）

`usage.py import-native` 用的 manifest：把 native session 的 turn（`native_id`）映射到 round 的
`request.json`。W2 6 份（T6/T6b/T7/V6/V6b/V7）、W3 6 份（T9/T11/T11b/V9/V11/V11b）、
W4 7 份（T13/T13b/T13c/T15/V13/V14/V16）。
**逐轮手工建立映射**这件事本身就是 **P16**；manifest 里被迫全填 `purpose=task` 是 **P19**。

> 全项目的 token 总账（含 hermes/opencode，从它们各自的 SQLite 核出）见
> `.orch/waves/W4/summary.md` §4.1 —— hermes/opencode 无需本目录的 manifest 也能计量。

## 4. `preflight/` —— 开工前的 skill 自检产物（17:1x–19:3x）

| 子目录 | 内容 | 关联 |
|---|---|---|
| `audit-20260919/` | 对 skill 自身的审计：`audit-findings.json` + 4 个 round + `crash-window` | P1–P4 的取证现场 |
| `forward-test/` | prepare/validate 的转发测试：`packet-{a,b,c,d}.json`、`known.json` | P2 |
| `socket-green/` | socket 路径的对照实验：`conflict.err/out` + 一个 round | P1 |
| `usage-green/` | `usage.py` 对照实验：`cross.err/out`、`native.json`、`result.json` | P15/P16 |
| `watch-x90zk5ag/`、`watch-fb2va6sf/` | `watch.py` 的真实运行状态：`watch.json`、`state.json`、`events/`、`reviews/`、`evidence/` | **P6**（`review_due` 噪声）|
| `skill-smoke-rounds/` | 16 个早期 smoke round（cwd 是 `/tmp/hd*`、`/tmp/pk-smoke`，非 mini-git）| pane-ID 校验问题的最早现场 |

## 5. `harnesses/` —— 验证者的对拍脚本与夹具

W1–W4 的验证者把对拍脚本、夹具仓库、原始输出放在 `/tmp/<名字>/`（任务书允许）。
这里按来源目录归档（**已剔除 `target/` 等构建产物**，见 `harnesses/MANIFEST.json` 的 kept/skipped 统计）：

| 目录 | 内容 | 对应发现 |
|---|---|---|
| `v16/`（含 `mutA/` `mutB/` 全量源码）| V16 的变异测试：mutA = 把 `fast_forward` 回退成 `flat_tree`；mutB = 用错误 oid 重建 index；`probe*/` 是 6 组夹具 | **W4-D1/D9**、**P18**（两个变异体最初共用 `CARGO_TARGET_DIR` → 假结论）|
| `t16/` | T16 的对拍：`diff.sh` + `repro/` + `diff/`（7 场景逐字段比较）| W4-D1 |
| `t15/` | T15 的 e2e 与 pack/idx 对拍 | W4-D2/D3、T15 交付 |
| `t13b/`、`t13c-codex`、`t13d3` | clone 无 tag / 空远端 / 多分支的夹具矩阵 | W4-D2/D8 |
| `t14b/` | HTTP `Accept` 头的原始 TCP 抓包脚本 | W4-D7 |
| `t11b/` | 42 格 symlink 夹具矩阵（switch/switch -f/reset 各 3 个场景）| **W3-D1/D2**、**P18** |
| `w3-t11-omp/` | T11 的 95 项 e2e + 300 例随机差分（`fuzz.py`、`e2e.py`、`e2e-work/`）| W3-D3/D4 |
| `t16b-mg/`、`t16b-probe/` | T16b 的「本地改动按路径判定」6 组夹具 + 边界探测 | W4-D9 |
| `g6/`、`g7/` | controller 复跑 symlink 矩阵的两个现场（**只有夹具、没有可归档的文本**，脚本在 `controller-correction.md` 里）| W3-D2 的仲裁 |

## 6. `mutants/v11b-mutant/` —— W3/V11b 的变异体

把 `src/worktree/materialize.rs` 的 `ancestor_barrier()` 守卫**删掉**后的完整源码副本
（V11b 用它证明自己的测试有灵敏度：两个变异体都被检出）。**这是 W3-D1（安全级缺陷）的复现素材**。

## 7. `release-p24/` —— release 专项测试的现场（**P23/P24 的原始证据**）

W4 收口后按用户要求补跑的「release 构建实际运行」专项测试。**结论：release 不可用** ——
`debug_assert!` 包住了带副作用的调用（P24），release 下整条被编译掉，`mg diff` / `mg merge`
进空循环 100% CPU 永久挂起；debug 构建 100% 正确。

| 子目录 | 内容 | 说明 |
|---|---|---|
| `scripts/gen.py` | 200 例随机语料生成器（单文件版，现场手写） | 最小复现的第 0 步 |
| `scripts/diff2.py` | 批量对拍：**debug 当基准**，逐例比 release vs debug，带 `timeout` 抓死循环 | 得出 3 死循环 / 12 输出不同 / 185 一致 |
| `scripts/one.py` | 单例三方差分：`debug mg` / `release mg` / 真实 `git` 逐字节比较 | 定位到「debug==git 恒成立，release 崩」 |
| `repro/old.txt`、`new.txt` | 触发死循环的最小文件对（32B / 18B） | 最小 CLI 复现的输入 |
| `repro/hang-old.bin`、`hang-new.bin` | 同上，未加换行的原始字节 | 对齐 `scripts/gen.py` 的字节级语料 |
| `repro/rel.out` | release `mg diff` 的输出 = **空**（被 `timeout` 杀掉，0 字节） | 死循环的直接证据 |
| `repro/dbg.out`、`git.out` | debug `mg diff` 与真实 git 的输出 —— **两者 sha256 完全相同**（`8bf0eed0a679`） | 证明缺陷只在 release profile |
| `repro/mrg*.out` | `mg merge` 三方路径的同类现场：`mrg2/3.out` 为空（挂死），`mrg3d.out` 是冲突提示 | 收敛到 `src/merge/three_way.rs:201` |
| `logs/rel-build.log` | `cargo build --release` 的耗时（0.06s，无 warning） | 编译本身没问题 |
| `logs/rel-test.log` | `cargo test --release` 全量日志 —— 卡在 `diff::myers::tests::random_scripts_are_valid` | **<卡死> 的第一现场** |
| `logs/rel-verify_*.log`（8 份） | 逐 target 的 release 运行日志（diff/merge_ff/threeway/materialize/... ） | 272 passed / 0 failed 的证据 |
| `logs/verify_diff.timeout.log` | `verify_diff` target 被 `timeout` 杀掉的日志 | 118s 无任何测试完成 |
| `logs/release-scan.txt` | **逐 target 扫描总表**：2 个 `exit=124 <卡死>`，其余 20 个全绿 | P23 的核心证据 |

```bash
# 复跑最小 CLI 复现（约 2 秒能看出卡死）
cd /tmp && rm -rf hang && mkdir hang && cd hang
R=/home/user/Projects/mini-git/target/release/mg
$R init . && printf 'b\n\n\n\na\n  c\na\na\n  c\n\nd\nb\ne\nd\nb\na\n' > f.txt
$R add f.txt && $R commit -m s
printf '  c\na\n  c\n  c\nd\na\n' > f.txt
timeout 6 $R diff   # release: exit 124；debug: 正常且与 git 逐字节相同

# 复跑 200 例对拍（依赖 build 出来的 debug + release 两个二进制）
python3 .orch/artifacts/release-p24/scripts/diff2.py
```

> ⚠️ 复跑 `cargo test --release` 一定要套 `timeout`，否则会永久挂住（P24 未修，按用户指示暂不修）。
> 直接跑 `target/release/deps/<test>-<hash>` 会因为 `CARGO_BIN_EXE_mg` 未设置而 13 项全 FAIL（不是缺陷）。

## 8. `notes/w4v/` —— 收口时的中间产物

`v13-head.md` / `v14-head.md` / `v15-head.md` / `common-tail.md`：写 W4 文档时从各轮
`result.json` 抽出的片段（不是证据本身，证据在 `.orch/rounds/`）。

---

## 哪些 `/tmp` 路径**没有**归档（已消失或与项目无关）

| 路径 | 原因 |
|---|---|
| `/tmp/v11-fail1`、`/tmp/gx` | W3-D1 的最小复现现场，**已被清理**；复现脚本完整保存在 `.orch/waves/W3/defects.md` 与 `T11b-symlink-safety/controller-correction.md` |
| `/tmp/w3-t11-omp/fuzz.py` 的运行产物 | 脚本已归档（`harnesses/w3-t11-omp/`），逐次运行的临时仓库属可再生产物 |
| `/tmp/v16/*-target`（约 1.2 GB）| 三个 `CARGO_TARGET_DIR`，纯构建产物，**刻意剔除**（P18 的教训就是它们被共用） |
| `/tmp/pal` | 与 mini-git 无关 |
| `/tmp/t*-prepare.json`、`/tmp/t1-prepare.err` | `prepare` 的中间输出，最终 request 已在 `.orch/rounds/` |

## 复跑方式

```bash
# 例：重跑 V16 的两个变异体（必须每个变异体一个独立 CARGO_TARGET_DIR —— P18）
cp -r .orch/artifacts/harnesses/v16/mutA /tmp/mutA && cd /tmp/mutA
CARGO_TARGET_DIR=/tmp/mutA-target cargo test --offline --test verify_merge_ff2   # 应当 FAIL

# 例：V11b 的变异体
cp -r .orch/artifacts/mutants/v11b-mutant /tmp/v11b-mutant && cd /tmp/v11b-mutant
CARGO_TARGET_DIR=/tmp/m-1 cargo test --offline verify_materialize                # 应当 FAIL（守卫被删）
```

> 归档里的脚本写死了当时的绝对路径（`/home/user/Projects/mini-git`、`/tmp/...`），
> 复跑时按需改路径；它们的作用是**证明当时怎么测的**，不是开箱即用的工具。
