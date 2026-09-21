# Wave W2 缺陷与观察清单

> 范围：W2（T5–T8 四个任务 + V5–V8 四个验证者 + T6b 返工轮）。
> **分类**：`产品缺陷`（mini-git 自身的行为与真实 git 不一致）/ `流程缺陷`（编排 skill 侧）。
> 跨 wave 的合并视图在 `.orch/SKILL-FINDINGS.md`（编号 P*），本文件用 `W2-*` 编号。

## 产品缺陷（mini-git）

| # | 发现者 | 现象 | 影响 | 处置 |
|---|---|---|---|---|
| W2-D1 | **V6（codex 验证者）** | `mg diff` 的 `--- `/`+++ ` 行在**路径含空格**时缺少 git 的 TAB 填充：git 输出 `--- a/a b.txt\t`，mg 输出 `--- a/a b.txt` | 路径含空格的仓库里 `mg diff` 与 `git diff` 逐字节不一致（其余行一致） | **T6b 返工轮**（omp）修复 `src/diff/unified.rs`；V6b 复验 |
| W2-D2 | T5（hermes 作者）自查上报 | `T5-odb/task.md` §3.2（tree mode 5 位 `40000`）与 §4.6（与 `git ls-tree` 逐字节一致）自相矛盾；真实 git 打印 `040000` | 若照 §3.2 实现，作者自己的 §4.6 验收必然失败 | 任务包**原样保留**（证据价值）；修正见 `ORCHESTRATION.md` C-14 |
| W2-D3 | T7（codex 作者）自查上报 | 初版单测用**单调递增时间戳**，导致「只按时间戳取最新共同祖先」的变异体 M1 **逃逸**（假绿） | 测试强度不足会让错误实现通过 | 作者改成「时间戳与拓扑相反 + criss-cross 同戳」后 M1 被 7 条用例检出；controller 把该教训写进 V7 任务书（P11 备注） |
| W2-D4 | T5（hermes 作者）自查上报 | 做 git 实验时误 `export GIT_CONFIG_GLOBAL/SYSTEM/NOSYSTEM`（P9 的环境变量泄漏） | 可能让**并发的**其它 agent 测试假红 | 作者自查后在本轮 shell `unset`，并改用 `Command::env` 就地隔离；未影响他人（本轮无人因此假红） |

**W2-D1 的最小复现**（V6 报告原文，已验证）：

```bash
mkdir /tmp/d && cd /tmp/d && git init -q -b main
printf 'line1\nline2\n' > 'a b.txt'; git add -A
git -c user.name=V -c user.email=v@e commit -qm base
printf 'line1\nCHANGED\n' > 'a b.txt'
git diff --no-renames > git.out
/home/user/Projects/mini-git/target/debug/mg diff > mg.out
cmp git.out mg.out      # → differ: char 75, line 3
```
证据留存：`.orch/waves/W2/T6-diff/verify-scratch/known-fail-space-in-path.txt`；
回归用例：`tests/verify_diff.rs::cli_space_in_path_tab_padding`（V6 写的，当前 `#[ignore]`）。

## 流程缺陷（编排 skill 侧）

| # | 现象 | 证据 | 对应 P 编号 |
|---|---|---|---|
| W2-S1 | **V5 的验证任务书漏了「变异测试」要求**，而 V6/V7/V8 都有。结果是四个验证者的**证据强度不可比**（V5 只有对拍、没有变异体） | `.orch/waves/W2/T5-odb/verify-task.md` vs `T6-diff/verify-task.md` §3/§4 | 新增 |
| W2-S2 | **T8 验证任务书指定的真值来源是错的**（`git ls-remote` 的 stdout 不是 pkt-line）；V8 自己实测后改用 `upload-pack --advertise-refs` | `.orch/rounds/W2/agent-orchestrator-xo5zp4pb/result.json` [B1] | **P11** |
| W2-S3 | **V6 任务书若照抄默认 `git diff` 会对拍出假 FAIL**：T6 的 rename 检测是明确非目标，而 `git diff` 默认开 renames。controller 在投递前补了「对拍请带 `--no-renames`」的备注才拦住 | `.orch/waves/W2/T6-diff/verify-task.md` 末尾 controller 备注 | 新增（与 P10 同源：任务包/非目标没写进验收口径） |
| W2-S4 | `prepare --parent-depth 1` 产出的 `depth` 是 **2**（不是 1）；controller 试错一次并删掉误建的 round 目录 | `agent-orchestrator-nl60jxdq`（depth 2，已删）→ `agent-orchestrator-i2muklue`（depth 1） | **P12** |
| W2-S5 | `agent prompt` 返回值里的 `agent_status` 是**投递前**的旧状态（本次 5/5 次都是 `idle`，`revision` 不变） | `.orch/artifacts/tools/orch-targets.txt` 各轮记录 | **P14** |
| W2-S6 | hermes **再次**在轮次结束后改写自己的私有 skill（本轮是验证者 V8，还新增了一个 reference 文件） | V8 面板 `💾 Self-improvement review: Skill 'contract-driven-module-delivery' patched ...`；W1 已有 D3 记录 | **P3** |
| W2-S7 | codex 的**原生审批风暴**：T7 作者 2 次、V6 验证者 4 次（其中 3 次由 `.orch/artifacts/tools/orch-autogrant.py` 按白名单半自动批准，1 次人工）。命令多为 `/tmp` 探测与临时仓库操作 | `.orch/artifacts/logs/orch-approvals.log`（逐条时间戳 + 命令） | **P5** |
| W2-S8 | 并发 wave 的**改动归因只能靠 mtime 窗口**：V5/V6/V7/V8 都只能「基线 sha256 差异 + 作者 result 时间窗」推断哪些文件属于自己 | 四份 result 的「白名单审计」段 | **P13** |

## 观察（不足以算缺陷，但值得记）

- **交叉验证确实抓到了真缺陷**：W1 抓到 gitlink（C-10）与 porcelain 排序（C-9），
  W2 抓到 diff 空格路径 TAB（W2-D1）。两次都是**作者 kind ≠ 验证 kind** 的组合。
- **作者自曝能力**：T5（规格冲突 + 环境变量泄漏）、T7（自己的测试假绿）都是作者**主动**写进 result 的。
- **验证者会质疑任务书**：V8 直接写「更正任务书」，V5/V6/V7 都主动说明了「按 controller 备注不判 FAIL」的边界。
- **`cargo test --offline` 在本 wave 期间从未因为并发写而变红**：验证者遇到的 2 次瞬时编译失败
  都被正确标注为「不可归因于本任务」并等 30 秒重试（W1 的共享 checkout 纪律有效）。
