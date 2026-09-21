# V9 —— 独立验证 T9（pack 读取：idx v2 + 对象头 + OFS/REF delta）

你是本轮的**验证者**（kind: omp），不是实现者。作者是 codex。
**不接受作者自述**，以真实 `git` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W3/T9-pack/task.md`（先读）
- 作者结果：**由 controller 在 prompt 里给出的 result 路径**（只读，不得修改）
- 作者可改文件：`src/odb/pack/{delta,idx,read,set}.rs`
- 开工前基线：`.orch/waves/W3/T9-pack/baseline.txt`

## 1. 写作用域（白名单）

- `tests/verify_pack.rs`（**新建**）
- `.orch/waves/W3/T9-pack/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件，尤其 `src/**`（只读）、`Cargo.toml`、`Cargo.lock`、
别人的 `tests/verify_*.rs`、`.orch/rounds/**`。发现缺陷不要代改，写进结论（要 controller round）。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline pack::
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_pack.rs`，零硬编码真值）

用**真实 git** 造 pack（`git gc --aggressive --prune=now`、`git repack -adf`、
`git repack -adf --window=50 --depth=50` 至少两种配置），断言：
1. `PackSet::open` 成功；`iter_oids()` 与 `git cat-file --batch-all-objects --batch-check`
   的 oid 集合**完全相等**（数量也要断言）。
2. 对**每一个** pack 对象：`PackSet::read` 的 `(Kind, payload)` 与 `git cat-file <type> <oid>`
   逐字节相等（不是抽样）。
3. **delta 链**：用 `git verify-pack -v <idx>` 输出定位 `chain length >= 2` 的对象，
   逐个断言内容正确；把 `git verify-pack -v` 的关键行贴进 result。
4. `PackIndex::lookup` 返回的偏移与 `git verify-pack -v` 里该 oid 的偏移一致。
5. **反例（必须失败，不是 panic）**：`.idx` 截断/改 magic/改 version、`.pack` 截断、
   对象 zlib 流损坏、delta copy 越界 → `Err(Corrupt)`（或 `Unsupported` 并在 result 说明）。
6. **不许假绿**：必须真的调 `minigit::odb::pack`；真值只能来自真实 git 进程与文件系统。

### (C) 防假绿
- 抽 1 条作者的用例，说明它的真值来自真实 git（不是硬编码）。
- 检查作者有没有「只测自己写的 `.idx` 再自己读」的自洽式假绿。

## 3. 判定
明确 `PASS` / `FAIL`，逐条列「场景 → 命令 → 关键输出 → 是否成立」。FAIL 必须给最小复现。

## 4. 真值来源必须自证（W2 教训 P11）
任务书里写的**具体命令只是提示**，不保证它真的是该真值的来源。开工前你必须自己确认：
例如 `git ls-remote` 的 stdout **不是** pkt-line；`git status --porcelain` 的分组顺序不是全局字节序。
如果你发现任务书给的来源是错的，**在 result 里写明「任务书 X 处有误，正确的是 Y」**，
并改用正确的来源继续验证。

## 5. 变异测试（本轮**所有**验证者都要做，不再是可选）
在**独立仓库副本 + 独立 `CARGO_TARGET_DIR`**（例如 `/tmp/vN-mutant`、`/tmp/vN-target`）上做，
**不要**改本仓库的 `src/**`、不要污染共享 `target/`。至少 2 个变异体，且必须包括：
1. 一个「把被测语义改错」的变异（例如去掉一条上界检查、颠倒一个条件）；
2. 一个「让实现恒返回某个平凡值」的变异。
断言：你的测试**必须 FAIL**（把失败用例名与关键报错贴进 result）。若某个变异体没被检出，
说明该断言是恒真的，**必须补测试**后再判 PASS。

## 上报

- result 路径：`prompt.txt` 给出的绝对路径。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 不要贴完整逐字稿；给「命令 + 关键输出 + 结论」。**明确区分**
  「FAIL」「未覆盖（说明原因）」「已确认的已知限制」。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1/W2 实测总结，必须遵守）

- **不要触发任何额外交互**：不启动引导/练习流程，不弹 Question/Ask 等人类输入。
  轮次结束的唯一标志是 result 文件写完。（omp 轮次结束后默认会弹「学习练习」→ 选「不要」。）
- **原生审批只按「单条」处理**：出现「1 Yes, proceed / 2 don't ask again / 3 No」时选 1，
  **不要选 2**。
- **不要 export 会被 git 读取的环境变量**（`GIT_AUTHOR_*` / `GIT_COMMITTER_*` / `GIT_CONFIG_*`）。
  用 `git -c user.name=... -c user.email=...` 或 `env VAR=... git ...`。
- **共享 checkout**：本 wave 另有 3 个验证者/作者在改别的子目录。编译错误若出自你白名单之外，
  记录并注明「不可归因于本任务」，等 30 秒重试，不要改别人的文件。
- **共享 `target/`**：并发 `cargo test` 会等文件锁（正常）。**变异测试必须用独立
  `CARGO_TARGET_DIR` 或独立副本**（W1 的 V2 因污染共享缓存返工过）。
- 对拍用的临时仓库全部放 `tempfile::tempdir()` 或 `/tmp/<你的名字>/`。
