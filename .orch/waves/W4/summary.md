# Wave W4 复盘（T13–T16：传输层 + fsck/gc 收口 + 5 个缺陷驱动返工 + 4 个交叉验证）

- 时间：2026-09-19 **21:38:58 → 22:45:25 +08:00**（第一份任务包发布 → 最后一个轮次完成，wall **1h06m27s**；
  agent 实际开工 21:42:36；controller 收口修复续到 ~23:00）
- 任务数：**3 个作者任务 + 3 个验证者 + 5 个返工/跟进轮 + 1 个跟进任务验证者 = 12 轮**
- 参与 kind：**codex / omp / opencode**（**hermes 本轮未上场**，见 §5 观察）
- 环境：herdr 用户既存 `default` 会话（**可见模式**），每 agent 一个可见 workspace（`--no-focus`），
  工作区策略 A（共享 checkout、目录互斥）
- 运行模型：**三段式** —— 3 个作者并行交付 → 3 个独立 space 的验证者并行判定 →
  缺陷驱动的返工链（其中 T16→V16→T16b 是一条「作者未覆盖的跨模块缺陷」三连）

| 任务 | 写作用域 | 作者 | 验证者 | 状态 |
|---|---|---|---|---|
| T13 `file://` 传输 + clone/fetch/push/pull | `src/transport/{local,negotiate}.rs`、`src/cli/{clone,fetch,push,pull}.rs` | **codex** | **omp**（V13） | ✅ |
| T14 smart HTTP（本机 `git http-backend`） | `src/transport/http.rs`、`tests/verify_http.rs` | **opencode** | **codex**（V14） | ✅（1 低严重度 FAIL）|
| T15 `mg fsck` / `mg gc` + 全量 e2e 收口 | `src/cli/{fsck,gc}.rs`、`tests/fsck_gc.rs` | **omp** | **opencode**（V15） | ✅ |
| T16 修 `mg merge` 嵌套路径快进（T15 的 D1） | `src/cli/merge.rs`、`tests/verify_merge_ff.rs` | **opencode** | **omp**（V16） | ✅ |
| T13b 修「远端无 tag 时 clone 失败」（T15 的 D2） | `src/transport/local.rs` | **codex**（复用 T13 会话） | — | ✅ |
| T16b 修 `mg merge` 的「本地改动」守卫（V16 的 F1） | `src/cli/merge.rs`、`tests/verify_merge_ff.rs` | **opencode** | — | ✅ |
| T14b 修 `Accept` 头（V14 的 FAIL） | `src/transport/http.rs`、`tests/verify_http.rs` | **opencode** | — | ✅（报 blocked，实现正确）|
| T13c 空远端 `mg clone` 对齐真实 git（V13 的 Q1） | `src/cli/clone.rs`、`src/transport/local.rs` | **codex** | — | ✅（报 blocked，实现正确）|

## 1. 逐任务结果

| 轮次 | agent / pane | round id | 提交 → 完成 | 用时 | 首位判定 | 备注 |
|---|---|---|---|---|---|---|
| T13 | codex / `w1E:p1` | `agent-orchestrator-eux_3b99` | 21:42:36 → 22:07:05 | 24m29s | — | 23 项新单测；**字节级 pkt-line 往返**（真实 ad 字节 → 解析 → 重编码 `assert_eq!`）；push 到 bare 远端 + 反向 git clone/push 回写全部通过 |
| T14 | opencode / `w1F:p1` | `agent-orchestrator-mk11hkd_` | 21:42:44 → 21:48:38 | 5m54s | — | 自带 TcpListener + 真 `git http-backend` CGI；9 项自测 |
| T15 | omp / `w1G:p1` | `agent-orchestrator-s4w2zqcy` | 21:42:52 → 21:54:35 | 11m43s | — | fsck 四段 + gc（**无 delta pack** + idx v2 全表 + `packed-refs` 与 `git pack-refs --all` 逐字节同）；**顺手抓出 3 个跨模块缺陷 D1/D2/D3** |
| V15 | opencode / `w1H:p1` | `agent-orchestrator-zzmj1afm` | 22:03:09 → 22:10:30 | 7m21s | **PASS** | `mg gc` 产物被 `git verify-pack`/`fsck`/`repack` 接受；6 类人工损坏方向一致；报 1 处已确认分歧（不可达 loose 损坏漏报） |
| T16 | opencode / `w1J:p1` | `agent-orchestrator-q0sxkvru` | 22:04:46 → 22:08:40 | 3m54s | — | 用 `odb.read_object(target_tree)` 替掉 `flat_tree(扁平路径)`；7 场景逐字段对拍 FAILS=0 |
| V16 | omp / `w1K:p1` | `agent-orchestrator-aedpxw6n` | 22:14:59 → 22:21:34 | 6m35s | **PASS** | 12 用例平行实现；**2 个变异体全部检出**；抓出 F1（守卫过严）与 F2（pull 冲突标签） |
| T13b | codex / `w1E:p1`（**复用 T13 会话**） | `agent-orchestrator-gr_8b9kt` | 22:14:32 → 22:19:20 | 4m48s | — | 通配 refspec 匹配 0 个 = 空操作；补 `warning: remote HEAD refers to nonexistent ref`；6 夹具与真实 git 对拍 |
| T16b | opencode / `w1P:p1` | `agent-orchestrator-c3ftayz5` | 22:23:37 → 22:32:27 | 8m53s | — | 全仓脏守卫 → **按路径**判定；6 组夹具 + 额外边界全部逐字段对齐 git |
| V13 | omp / `w1M:p1` | `agent-orchestrator-bc1br750` | 22:21:29 → 22:30:29 | 9m00s | **PASS** | T13 交付 PASS；**报 blocked 2 项**（空远端 clone 与 git 冲突、2 条过期 `#[ignore]`）；发现任务书 3 处描述与实测不符 |
| V14 | codex / `w1N:p1` | `agent-orchestrator-6e6wzvj0` | 22:21:53 → 22:34:52 | 12m59s | PASS（1 低严重度 FAIL） | 抓 `Accept: application/x-git-git-upload-pack-advertisement` 多一层 `git-`（真实 git 发 `*/*`） |
| T14b | opencode / `w1Q:p1` | `agent-orchestrator-gvvgao3_` | 22:37:33 → 22:41:38 | 4m54s | — | 改 `Accept` 为 `*/*`；**卡在 V14 文件里 2 条 characterization 断言** → 报 blocked |
| T13c | codex / `w1R:p1` | `agent-orchestrator-t2z2r0_i` | 22:37:57 → 22:45:25 | 7m28s | — | 空远端 clone 对齐 git（exit 0 + 逐字 warning + 只留 `.git`）；同样卡在 V13 文件里 1 条旧断言 → 报 blocked |

首轮通过率：**3/3 作者交付 success；3/3 验证者判定 PASS**（V14 附 1 条低严重度 FAIL，走 T14b）。
但本轮真正的产出是**自查与交叉验证抓出的跨模块缺陷**：5 个返工轮里 3 个（T13b/T16/T16b）
源自 T15 作者的自查与 V16 的复核，2 个（T14b/T13c）源自验证者。

## 2. 核心指标（§6）

| 指标 | 数值 | 说明/证据 |
|---|---|---|
| 协议有效率 | **12/12 = 100%** | 12 个 round 的 `protocol.py validate --check-files` 全 exit 0；`job_id`/`round_id`/result 路径零错配 |
| **假绿率** | **0** | 各轮声称的测试数量与结论，controller 独立复跑全部成立；V16 的变异测试 **2/2 检出**；V15 明确复核了作者用例「无自洽式假绿」 |
| 越界率 | **0 个文件** | V13 按 wave 级基线（181 文件）逐 sha256 核对：T13 六文件恰好、`Cargo.toml`/`Cargo.lock`/`src/odb/**`/别人的 `tests/**` 未变；V16 明确只增 `tests/verify_merge_ff2.rs`；`check-freeze.sh` 全程 drift 0 |
| 接口漂移次数 | **0** | `scripts/check-freeze.sh` → `checked 21 file(s), drift 0`（多轮多 agent 各复跑一次）。FREEZE 由 v0.5 → **v0.6**（controller 删除死代码 `error::todo`）→ **v0.7**（全仓 `cargo fmt --all`，47 文件、纯空白；C-29），公共 API 零改动 |
| blocked 处理 | 协议级 blocked **3**（V13 / T14b / T13c） | 全部是**「门禁测试所有权」类**：child 按写作用域纪律不许改别人的测试文件，于是报 blocked 并给出精确修法。问题具体、**零越界改**；controller 用 C-24 / C-27 / C-28 **直接接住**（不派轮），三例全部记账为「blocked 但交付完整」 |
| 交叉验证真实率 | **4/4 真实执行** | V13/V14/V15/V16 都新建了独立 test target（`verify_transport` / `verify_http_http` / `verify_fsck_gc` / `verify_merge_ff2`），真值零硬编码，全部含变异或反例测试 |
| 并行加速比 | **作者段 3.51×**；验证段 1.13×；返工段 0.74× | Σ作者 42m06s / 作者段 wall 11m59s（21:42:36→21:54:35）；Σ验证 35m55s / 验证段 wall 31m43s（22:03:09→22:34:52，被「等作者交付 + codex 审批」拖住）；Σ返工 29m57s / 返工段 wall 40m39s（22:04:46→22:45:25，串行等待为主）|
| 返工轮次占比 | **6/12 = 50%** | 5 个返工（T13b/T13c/T14b/T16/T16b）+ 1 个跟进验证（V16）。W4 是**缺陷驱动**的 wave：集成收口期（传输 + fsck/gc）暴露的跨模块缺陷数量远高于 W3 |
| token 用量（原生计数） | 见 §4 | codex/omp 有适配器；opencode 无（P15） |

## 3. 验证者的独立发现（本轮最大价值）

1. **W4-D1（跨模块，真缺陷）**：`mg merge` 快进在**含子目录**的仓库里必然失败（`fatal: corrupt tree entry name: "dir/b.txt"`）。
   由 **T15 的作者**在 e2e 收口自查时发现 → T16 修复 → V16 独立对拍 PASS。
2. **W4-D2（真缺陷）**：远端**没有任何 tag** 时 `mg clone` 直接 fatal（clone 无条件请求 `+refs/tags/*`）。
   同样由 T15 作者发现 → T13b 修复（通配 refspec 匹配 0 个 = 空操作）。
3. **W4-D3（并行中间态）**：T15 报告 `mg push` 拒绝 bare 远端与新建分支；**V13 复核时已好** ——
   T15 观测到的是 T13 尚在飞行中的中间态（共享 checkout）。归因修正见 `defects.md` 的 S 表。
4. **W4-D7（低严重度，真缺陷）**：`Accept` 头多一层 `git-`。V14 用「抓实际发出的原始请求行 + 与真实 git 客户端自己的请求逐字对拍」双证据判定 ——
   作者 9 个自测**完全没覆盖 Accept**（典型「断言盲区」，不是假绿）。
5. **W4-D8（真缺陷）**：空远端 `mg clone` 报 Err，真实 git exit 0 + `warning: You appear to have cloned an empty repository.`。
   V13 直接改写任务书里的错误前提，controller 裁决**跟随真实 git**（C-24）→ T13c。
6. **W4-D9（真缺陷）**：`mg merge` 的本地改动守卫是**全仓脏即拒绝**，真实 git 只拒绝「会被本次合并覆盖」的路径。
   V16 用最小复现把 F1 与 T16 的修复严格区分开 → T16b 改成按路径判定（6 组夹具 + 额外边界全对齐）。
7. **W4-D10 / F2（已确认分歧）**：`mg fsck` 不扫不可达 loose 对象损坏（V15）；`mg pull` 冲突标记的 theirs 标签用 `refs/remotes/origin/main`
   而 `git pull` 用 oid（V16，且 `git merge <ref>` 的标签与 mg 完全一致）→ 均记为已知限制。
8. **P18 复现（方法论）**：V16 的两个变异体最初**共用同一个 `CARGO_TARGET_DIR`**，mutB 复用了 mutA 的编译产物，
   症状与 mutA 完全相同（**假结论**）；改成每变异体独立 target 后症状才正确区分 —— 与 W3 的 P18 同源（结论必须自证夹具与制品）。

## 4. token 用量（原生计数，不做字节估算）

`usage.py import-native` 导入的 7 个 manifest（`.orch/artifacts/usage-manifests/`，codex/omp 各有原生适配器）：

| 轮 | host | calls | input | cached | output |
|---|---|---|---|---|---|
| T13 | codex | 77 | 10,872,603 | 10,790,144 | 135,156 |
| T13b | codex | 46 | 4,168,960 | 3,980,928 | 37,642 |
| T13c | codex | 64 | 5,965,283 | 5,883,648 | 76,505 |
| V14 | codex | 45 | 3,894,548 | 3,837,184 | 83,399 |
| T15 | omp | 120 | 20,425,299 | 20,305,152 | 144,926 |
| V16 | omp | 57 | 5,713,520 | 5,644,928 | 71,540 |
| V13 | omp | 87 | 10,795,713 | 10,675,584 | 95,286 |
| **合计（7/12 轮）** | | **496** | **61,835,926** | **61,117,568（98.8%）** | **644,454** |

### 4.1 全项目累计（W1–W4，四个 host 全部从各自的原始会话库核出）

W4 收口时补做了一次「跨 host 总账」：不再依赖 `usage.py` 的适配器（它只认 codex/omp，**P15**），
而是直接读四个 host 自己的会话存储 —— codex `~/.codex/sessions/**/*.jsonl`、
omp `~/.omp/agent/sessions/**/*.jsonl`、hermes `~/.hermes/state.db::session_model_usage`、
opencode `~/.local/share/opencode/opencode.db::message.data.tokens`（按 `directory = .../mini-git` 过滤）。

| host | 会话数 | calls | input（含 cached） | cached | output | 说明 |
|---|---|---|---|---|---|---|
| **codex** | 12 | 1,456 | **170,989,728** | 168,603,392 | 1,644,926 | 含 **controller 主会话** |
| ├ controller（我） | 1 | 773 | 104,882,473 | 103,533,568 | 591,094 | 18:49 起，全程主线 |
| └ codex agent 轮 | 11 | 683 | 66,107,255 | 65,069,824 | 1,053,832 | T1/T9/V11/V11b/T13/T13b/T13c/V14 等 |
| **omp** | 11 | 810 | **117,291,859** | 116,367,104 | 1,092,710 | W1–W4 全部 omp 轮 |
| **hermes** | 7 | 544 | **70,834,498** | 70,037,120 | 608,216 | 含 1 个 `source=subagent` 的子会话 |
| **opencode** | 10 | 527 | **72,642,866** | 71,655,680 | 247,338 | 按 `directory` 过滤 |
| **合计** | **40** | **3,337** | **431,758,951** | **426,663,296（98.8%）** | **3,593,190** | |
| └ 其中 agent 侧（不含 controller） | 39 | 2,564 | 326,876,478 | 323,129,728 | 3,002,096 | |

**读法**：
1. `input` 已含 `cached`，不要相加；缓存命中率 98.8% 是「长会话反复带同一段上下文」的必然结果。
2. **controller 自己花掉 24%**（104.9M / 431.8M）—— 编排开销的大头不是 agent，是「读所有人的结果、做裁决、写文档」的那个人。
3. 最重的一轮是 omp 的 **T15**（120 calls / 20.4M input），最轻的是 opencode 的 T14（5m54s 交付）。
4. hermes 侧出现了 **1 个 `source=subagent` 的会话**（21:07:45，29 calls / 0.29M input）——
   说明 hermes 在 W3/V10 那一轮**自发做了嵌套委派**；这与 `SKILL-FINDINGS.md` §2「嵌套委派未做」的结论不符，
   需要修正（我们只观测到**一个**子会话，且它不出现在任何 round 的 result 里，属协议外的自主行为）。

**未测**：T14 / T16 / T16b（opencode）、V15（opencode）—— opencode 无原生适配器（**P15**）。
缓存命中率 98.8% 说明「同一 native 会话续轮」（T13→T13b）成本极低（46 calls 仅 4.2M input）。
验证轮与返工轮在账本里只能记 `purpose=task`（**P19**）：同一 native session 的续轮是不同 `native_id`，
所以**成本可以精确切开**，但「按轮型聚合」在数据模型层做不到。

## 5. 注入场景与观察

| 场景 | 是否触发 | 实际行为 | 与期望的差异 |
|---|---|---|---|
| S5 越界（改别人的文件） | **3 次机会均未发生** | V13/T14b/T13c 都撞上「别人的测试文件钉住旧行为」，三例全部**报 blocked + 给出精确修法**，零越界 | 无；这正是 S5/S9 想测的行为 |
| S9 需 controller 裁决 | **触发 3 次** | V13 的 Q1（任务书前提与真实 git 冲突）→ 裁决跟随真实 git；T14b/T13c 的 Q1（旧断言）→ controller 直接改 | 无；但「测试所有权」债务**连续第二个 wave**成为返工主因（W3 是 P17，W4 是 P20） |
| S4 中途 kill / S8 结果改写 | 未注入 | — | 留给下一 wave |
| hermes 缺席 | — | W1–W3 都有 hermes，W4 只有 codex/omp/opencode。**这是排期选择**（hermes 在 W3 的 V10 完成度最高但速度最慢），不是 hermes 故障 | 骨架的「4 kind」在这一 wave 退化为 3 kind，横向对比需在 `SKILL-FINDINGS.md` 注明 |

## 5.5 收口后补测：release profile（2026-09-20，用户要求）

`cargo test --release --offline` → **lib 与 `verify_diff` 两个 target 卡死（exit 124）**，
其余 20 个 target **272 passed / 0 failed / 1 ignored**。release 二进制在 200 例随机语料上
**3 例死循环、12 例输出与真实 git 不同**（debug 全部与 git 一致）。

根因：`src/diff/myers.rs::change_compact` 的 8 处 `debug_assert!` 包住了**带副作用的调用**，
release 下被整条编译掉 → 游标不推进 → 空循环 / 压缩未执行。
这是 **P23（门禁只跑 debug）+ P24（`debug_assert!` 副作用）**，四个 wave 的所有门禁都看不见它。
详见 `.orch/waves/W4/defects.md` 的 W4-D12/W4-S12 与 `.orch/SKILL-FINDINGS.md` P23/P24。

## 6. 结论与 skill 缺陷清单

- **可复现的 skill 缺陷（本轮新增）**：
  - **P20（新）** 过期的 `#[ignore]` 不会自己失效 —— W4 一口气清了 4 处，其中 `tests/verify_diff.rs::cli_space_in_path_tab_padding`
    从 **W2/T6b 修好后**就一直「静默关闭」，跨了两个 wave 才被发现；
  - **P21（新）** 并发 wave 里，作者在自己的构建上观测到的「peer 的缺陷」可能是**中间态**（W4-D3 实例），
    归因链上没有人能区分「最终交付缺陷」与「飞行中的中间态」；
  - **P19 / P15 / P17 / P18 全部在 W4 再次出现**（未修 → 必然复发）：轮型词表、跨 kind 成本、测试所有权、真值夹具自证。
- **本轮最有价值的产出**：`mg` 的传输层与完整性收口（fsck/gc）在**一个 wave 内闭环**，
  且 5 个返工轮全部由**真实 git 对拍**驱动，而不是靠 agent 自述。
- **下一 wave 的模板改动**：
  1. 任务书必须写「门禁测试的**归属**」一节：本 wave 会动红的旧测试由 controller 预先列出并授权（否则必然产生 blocked）；
  2. 验证任务书必须写「作者可改文件在**我验证期间**是否仍在变动」，并要求记录制品哈希（V16 已自发做了，要固化成条款）；
  3. 变异测试条款补一句：「每个变异体必须独立 `CARGO_TARGET_DIR`，且用 CLI 原始输出二次确认」（P18 的最小落地形式）；
  4. 门禁一节必须补 `cargo fmt --all -- --check` —— 4 个 wave 没有任何一轮跑过 rustfmt，格式债务一路累积到收口（C-29）。
