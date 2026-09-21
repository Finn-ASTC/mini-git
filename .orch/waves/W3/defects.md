# Wave W3 缺陷与观察清单

> 范围：W3（T9–T12 四个实现任务 + V9–V12 四个验证者 + T11b 返工轮 + V11b 复验轮）。
> **分类**：`产品缺陷`（mini-git 自身行为与真实 git 不一致）/ `流程缺陷`（编排 skill 侧）。
> 跨 wave 的合并视图在 `.orch/SKILL-FINDINGS.md`（编号 P*），本文件用 `W3-*` 编号。

## 产品缺陷（mini-git）

| # | 发现者 | 现象 | 影响 | 处置 |
|---|---|---|---|---|
| W3-D1 | **V11（codex 验证者）** | 物化**删除**路径跟随 symlink 祖先：`a` 是指向工作区外的 symlink 时，`mg switch` / `checkout -f` / `reset --hard` 会把工作区外的 `outside/b/c`、`outside/b` **删掉** | 数据丢失（安全级）。`-f`/`--hard` 下连「与 index 一致」这个前提都不需要 | **T11b 返工**（omp）：新增 `ancestor_barrier()` 逐段 `symlink_metadata`，`remove_worktree_entry` / `prune_empty_dirs` 先过守卫；V11b 复验 |
| W3-D2 | **V11** | 物化**写**路径遇 symlink 祖先且**顺 symlink stat 不到目标**时，mg exit 1 拒绝，真实 git exit 0（换成真目录后写入工作区内） | 与 `T11 §3.3`「与 git 逐字节一致」冲突 | T11b 修复（只放行「目标已在 index 且 stat 不到」这一格；未跟踪新路径仍拒绝）。**注意真值口径**见 C-21 与 `T11b-symlink-safety/controller-correction.md`（v2） |
| W3-D3 | **T11 作者**（随机差分自查） | 祖先段是**普通文件**时 mg 报 `fatal: io error: Not a directory (os error 20)`；真实 git 在 force 下是 exit 0 跳过 | 崩溃式错误（非 panic，但形态与 git 差很远） | T11b 顺带修掉：进 42/42 一致集 |
| W3-D4 | **T11 作者**（300 例随机差分） | 另外 4 类真实偏差：同提交捷径（目标 == HEAD 且非 `-f` 时 git 只改 HEAD）、空目录 prune、两类 dirty 判定 | 与 git 不一致 | 轮内修掉并对拍（`/tmp/w3-t11-omp/fuzz.py`） |
| W3-D5 | **T12 作者**（对拍自查） | **真实 git 侧**的陷阱：`.git/index` 的 cache-tree（`TREE` 扩展）过期时，真实 git 会**静默提交旧 tree** | 双向互操作的隐性坑：mg 改了 index 若不丢 `TREE`，git 后续命令会读到过期树 | T12 在 `add`/`rm` 等改 index 后丢弃 `TREE`，并在 `commit` 后等价实现 git 的 `cache_tree_update` |
| W3-D6 | **V10（hermes 验证者）** | `merge.conflictstyle=zdiff3` 时 mg 输出 diff3 形态（公共行留在 marker 内），真实 git 把公共行挪到 marker 外 → 逐字节不同。作者自述「支持 diff3/zdiff3」不成立 | 仅影响 `zdiff3`（v1 非目标）；任务书只要求 diff3，该项逐字节 PASS | **不修**：按 C-22 记为已确认的已知限制，写进 README 的非目标 |
| W3-D7 | **V9（omp 验证者）** | 作者的「真 git 用例」以 `if !gitkit::git_available() { eprintln!("skipping"); return; }` 开头 —— 在没有 git 的环境里会**静默通过** | 假绿风险（测试存在但永不失败） | V9 自己改成硬失败；W4 模板已加「不许 skip 即通过」的口径要求 |
| W3-D8 | **V9** | 覆盖缺口：`.idx` 大 offset 表项（≥ 2^31）**正分支**未覆盖（本机造不出 > 2 GiB pack），只有合成的自洽式用例 | 覆盖缺口（非缺陷） | 记入未覆盖项；`git fsck` 的 crc32 校验也留给 T15 |

**W3-D1 的最小复现**（V11 报告原文，controller 已复跑）：

```bash
cd /tmp && rm -rf v11-fail1 && mkdir -p v11-fail1/outside/b && cd v11-fail1
git init -q -b main r && cd r
mkdir -p a/b && echo v1 > a/b/c && git add -A && git -c user.name=t -c user.email=t@e commit -qm one
git -c user.name=t -c user.email=t@e checkout -q -b prune && git rm -q a/b/c \
  && git -c user.name=t -c user.email=t@e commit -qm prune
git -c user.name=t -c user.email=t@e checkout -q main
echo arbitrary > /tmp/v11-fail1/outside/b/c && rm -rf a && ln -s /tmp/v11-fail1/outside a
/home/user/Projects/mini-git/target/debug/mg reset --hard prune    # 修复前：outside/b/c 被删
```

**W3-D2 的夹具矩阵**（controller 独立复跑，`/tmp/g6` 用真实 git、`/tmp/g7` 用 mg）：

| 顺 symlink 祖先能否 stat 到目标 | 真实 git 非 force | 真实 git force | mg（T11b 后） |
|---|---|---|---|
| 不能（outside 为空） | **exit 0**，换成真目录写入工作区内 | exit 0，同左 | 一致 |
| 能（outside/b/c 存在） | **exit 1** 拒绝 | exit 0，换成真目录写入 | 一致 |

## 流程缺陷（编排 skill 侧）

| # | 现象 | 证据 | 对应 P 编号 |
|---|---|---|---|
| W3-S1 | **门禁测试所有权缺失**：一次 wave 出现 3 例「旧世界断言」变红（`tests/interop/smoke.rs`、`tests/verify_worktree.rs`、V11 的 characterization），其中 T12 因此把 result 报成 `blocked`（交付其实是完整的） | `.orch/rounds/W3/agent-orchestrator-7j820bx3/result.json` §4；`ORCHESTRATION.md` C-18/C-19/C-20 | **P17** |
| W3-S2 | **「真值」缺夹具就不是真值**：V11 的 FAIL 2 与 controller 的第一版「撤销」更正**两边都实测正确**，差别只在 `outside/` 空不空；controller 差点按错前提下发返工，V11b 也会用错真值判假红 | `T11b-symlink-safety/controller-correction.md`（v2）；T11b 的 42 格矩阵 | **P18**（本轮最重要） |
| W3-S3 | `agent start` 在既存会话里**第二次**命中超时（命令已输入未执行）；按文档配方 `send-keys enter` + `agent rename` 恢复成功 | T11b 启动记录（`submission_evidence`）；`SKILL-FINDINGS.md` P4 | P4 |
| W3-S4 | codex 原生审批风暴仍是唯一吞吐瓶颈：W3 用 `.orch/artifacts/tools/orch-autogrant.py` 半自动批准（V11 一轮内 ≥ 4 次），逐条记 `.orch/artifacts/logs/orch-approvals.log` | `.orch/artifacts/logs/orch-approvals.log` | P5 |
| W3-S5 | **复用同一 native 会话做复验轮可行**（V11 → V11b 同一 codex 会话，新 `round_id` + 新 `result_path`）：身份不串、上下文连续，省掉一次冷启动 | `.orch/rounds/W3/agent-orchestrator-mldbvlhk/` | 新增（好消息，建议写进 skill） |
| W3-S6 | **返工轮的 `baseline.txt` 无法隔离该轮改动**：T11b 用的是 wave 级快照，`drift.sh` 一次性列出 20+ 文件；真正的归因只能靠 **mtime 窗口** | `scripts/drift.sh .orch/waves/W3/T11b-symlink-safety/baseline.txt`；`find src -newermt '21:19'` 只列出 `materialize.rs` | **P13** 再现 |
| W3-S7 | hermes **第三次**在轮次结束后改写自己的私有 skill（本轮 V10，21:18 写了 `~/.hermes/skills/contract-driven-module-delivery/SKILL.md`） | 文件 mtime 21:18 与该轮结束时间 21:18:21 吻合 | **P3** |
| W3-S8 | 任务书允许「任务包前提被实测推翻」：T11b **没有照做**，而是先做 git 实验、在 result 里报冲突并给矩阵 | `agent-orchestrator-d8v5yjvq/result.json` §真实 git 真值 | 新增（已固化进 W4 模板） |
