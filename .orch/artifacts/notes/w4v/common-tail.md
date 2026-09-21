
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
