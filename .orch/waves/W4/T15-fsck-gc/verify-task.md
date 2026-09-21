# V15 —— 独立验证 T15（`mg fsck` / `mg gc` + 全量 e2e 收口）

你是本轮的**验证者**（kind: opencode），不是实现者。作者是 omp。
**不接受作者自述**，以真实 `git` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W4/T15-fsck-gc/task.md`（先读）
- 作者结果：**由 controller 在 prompt 里给出的 result 路径**（只读，不得修改）
- 作者可改文件：`src/cli/{fsck,gc}.rs`、`tests/interop/fsck_gc.rs`
- 开工前基线：`.orch/waves/W4/T15-fsck-gc/baseline.txt`（由 controller 在 wave 启动时拍）

## 1. 写作用域（白名单）

- `tests/verify_fsck_gc.rs`（**新建**）
- `.orch/waves/W4/T15-fsck-gc/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件：`src/**`（只读）、`Cargo.toml`、`Cargo.lock`、
作者的 `tests/interop/fsck_gc.rs`（只读）、`.orch/rounds/**`。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline fsck
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_fsck_gc.rs`，零硬编码真值）

**`mg gc` 的产物必须由真实 git 来判定**（mg 自读自写不算证据）：

1. 造一个**只有 loose 对象**的真实 git 仓库（`git repack`/`git gc` 之后 `git unpack-objects`
   或直接用小仓库多提交），`mg gc` 之后断言：
   - `git fsck --no-progress` 无 error（允许 dangling，dangling 不算错，与 git 一致）；
   - `git log --all --oneline` 与 gc 前**逐字节相同**；`git cat-file -p <每个 oid>` 全正常；
   - `git verify-pack -v <mg 生成的 .idx>` 通过（这是「mg 写的 pack 合法」的硬证据）；
   - `objects/` 下 loose 对象数**确实下降**（给出前后数字）；
   - `packed-refs` 格式：真实 git 能解析（`git show-ref`/`git for-each-ref` 结果不变）。
2. **幂等**：连续两次 `mg gc` → 第二次后 `git fsck` 仍无 error，`git log --all` 不变；
   （若第二次产生了新的 pack，说明不幂等，必须报出来）。
3. **`mg fsck` 的判定方向必须与真实 git 一致**（不是逐字节，是结论方向）：
   - 干净仓库（loose + pack 混合、含 annotated tag）→ `mg fsck` exit 0，且**真实 git fsck 也干净**；
   - **至少 6 种人工损坏**，逐个断言 `mg fsck` **exit 非 0 且报到 stderr**：
     ①loose 对象字节被改；②loose 对象被截断；③pack 文件被截断；④`.idx` 的 fanout/oid 表被改；
     ⑤tree entry 的 oid 不存在；⑥commit 的 parent oid 不存在 / `tree` 行缺失。
     每种都要同时贴**真实 `git fsck` 对同一损坏的输出**，证明「git 认为坏、mg 也认为坏」。
   - **反例的另一面**：dangling 对象（`git hash-object -w` 造一个没人引用的 blob）→
     `mg fsck` **必须 exit 0**（与 git 一致）。
4. **e2e 收口**：不信作者的 e2e，自己写一条最短路径：
   `init → hash-object → add → status → commit → log → tag → branch → switch → diff → merge(冲突) →
   reset → checkout → clone(file://) → fetch → push → fsck → gc`，
   每步与真实 git 对拍（平行仓库），比较 `git log --oneline`、`git status --porcelain`、
   `git ls-files --stage`。若某条命令本轮尚不可用（T13/T14 并行中），注明「未覆盖 + 原因」。

### (C) 不许假绿

- 抽 1 条作者的用例，说明它的真值来自真实 git（不是硬编码、不是 mg 自读自写）；
- 检查作者是否用「mg 读回自己写的 pack」当合法性证据（自洽式假绿），是则判 FAIL。

## 上报

- result 路径：`prompt.txt` 给出的绝对路径（本轮是新 round，**不要覆盖任何已有 result**）。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 不要贴完整逐字稿；给「命令 + 关键输出 + 结论」。**明确区分**
  「FAIL」「未覆盖（说明原因）」「已确认的已知限制」。

## 真值来源必须自证（W2/W3 教训 P11）

任务书里写的**具体命令只是提示**，不保证它真的是该真值的来源，也不保证它断言的 git 行为是对的。
开工前你必须自己用真实 git 确认：例如 `git ls-remote` 的 stdout **不是** pkt-line；
`git upload-pack --advertise-refs` 的输出**开头有 service 行**（HTTP 场景）；
`git status --porcelain` 的分组顺序不是全局字节序。
如果你发现任务书给的来源/预期行为是错的，**在 result 里写明「任务书 X 处有误，正确的是 Y」**，
并改用正确的来源继续验证。

## 门禁归属（W3 教训 P17）

本 wave 有 3 个作者在同一 checkout 并行改文件。`cargo test --offline` 里若有 target 变红：
先判断该 target 是否属于你的被验证对象。**不属于你的**，记录后注明「不可归因于本任务」，
等 30 秒重试；**不要去改别人的文件**，也不要因此判 FAIL。
若你发现某个断言属于「按里程碑已过期」的 characterization 用例（实现正确、断言过时），
**不要改它**：在 result 里报 `blocked` 级别的问题描述 + 最小修法，交 controller 裁决。

## 变异测试（本轮**所有**验证者都要做）

在**独立仓库副本 + 独立 `CARGO_TARGET_DIR`**（例如 `/tmp/vN-mutant`、`/tmp/vN-target`）上做，
**不要**改本仓库的 `src/**`、不要污染共享 `target/`。至少 2 个变异体，且必须包括：
1. 一个「把被测语义改错」的变异（例如去掉一条上界检查、颠倒一个条件、把 prefix 匹配改成等值匹配）；
2. 一个「让实现恒返回某个平凡值」的变异（例如恒返回 `Ok(vec![])`）。
断言：你的测试**必须 FAIL**（把失败用例名与关键报错贴进 result）。
若某个变异体没被检出，说明该断言是恒真的，**必须补测试**后再判 PASS。

## 发现缺陷时

**不要代改**（`src/**` 只读）。写进 result：最小复现（能一行行照着跑）、期望行为（真实 git 的输出）、
实测行为、影响面。若某个缺陷需要 controller 决策（例如「任务书与真实 git 冲突」），
标为 `blocked` 并给出两个可选修法。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1/W2/W3 实测总结，必须遵守）

- **不要触发任何额外交互**：不启动引导/练习流程，不弹 Question/Ask 等人类输入。
  轮次结束的唯一标志是 result 文件写完。（omp 轮次结束后默认会弹「学习练习」→ 选「不要」。）
- **原生审批只按「单条」处理**：出现「1 Yes, proceed / 2 don't ask again / 3 No」时选 1，
  **不要选 2**。
- **不要 export 会被 git 读取的环境变量**（`GIT_AUTHOR_*` / `GIT_COMMITTER_*` / `GIT_CONFIG_*`）。
  用 `git -c user.name=... -c user.email=...` 或 `env VAR=... git ...`。
- **共享 checkout**：编译错误若出自你白名单之外，记录并注明「不可归因于本任务」，等 30 秒重试。
- **共享 `target/`**：并发 `cargo test` 会等文件锁（正常）。**变异测试必须用独立
  `CARGO_TARGET_DIR` 或独立副本**（W1 的 V2 因污染共享缓存返工过）。
- 对拍用的临时仓库全部放 `tempfile::tempdir()` 或 `/tmp/<你的名字>/`。
- 起监听端口用 `127.0.0.1:0`（系统分配）并打印实际端口，避免与其它 agent 抢固定端口。

## 补充条款（W3 教训，v2 模板）

- **不许「skip 即通过」**：你的测试或作者用例里，凡是在环境缺失（`git` 不存在、端口不可用等）时
  `return` 而不失败的写法，一律视为假绿，必须在 result 里点名（W3/V9 实例）。
- **夹具必须写全**：任何「真实 git 会 X」的结论都要附**夹具的完整状态**（文件是否存在、权限、大小写等），
  否则该结论不可复现（W3/P18：同一句话在两个夹具上结论相反）。
- **可质疑自己的上一轮**：如果你是续轮（复验），发现上一轮自己的结论被夹具误导，直接说明并更正，
  这不算「丢面子」，是本轮最重要的产出之一。
