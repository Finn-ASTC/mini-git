# V8 —— 独立验证 T8（pkt-line）

你是本轮的**验证者**（kind: hermes），不是实现者。作者是 opencode。
**不接受作者自述**，以真实 `git` 产出的字节流为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W2/T8-pktline/task.md`（先读）
- 作者结果：`.orch/rounds/W2/agent-orchestrator-1matyqhj/result.json`（只读，不得修改）
- 作者可改文件：`src/transport/pktline.rs`
- 开工前基线：`.orch/waves/W2/T8-pktline/baseline.txt`

## 1. 写作用域（白名单）

- `tests/verify_pktline.rs`（**新建**）
- `.orch/waves/W2/T8-pktline/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件，尤其 `src/**`（只读）、`Cargo.toml`、别人的 `tests/verify_*.rs`。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline transport::
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_pktline.rs`，零硬编码真值）

1. **解析真实 git 输出（硬标准）**：在 temp dir 建仓库，用
   `std::process::Command` 拿真实 git 产出的 pkt-line 字节流，例如：
   - `git ls-remote <path>` 的 stdout（含 flush-pkt 结尾）；
   - 或 `git upload-pack --stateless-rpc --advertise-refs <bare-repo>` 的 stdout。
   用**你写的** `minigit::transport::pktline::read_pkt` 逐帧解析，断言：
   - 每一帧 `encode_pkt(payload)` 能**逐字节重建**原始帧；
   - 最后一帧是 `Pkt::Flush`；
   - 所有 payload 拼起来后，能被你按 git 的 ref advertisement 文本格式解析出至少一个 ref
     （证明你没有把帧切错）。
2. **构造边界**：`0004`（空 Data）、`0000`/`0001`/`0002`、最大 payload（65516）、
   超长（长度字段声明 > 65520）、`0003`、非十六进制 `zzzz`、截断（声明 8 字节只给 2 字节）、
   连续多帧一次写入后逐帧读出（游标正确）。
   非法输入必须 `Err(Error::Protocol)`，**不得 panic、不得返回部分数据**。
3. **写方向**：`write_pkt` 的输出被 `read_pkt` 读回必须相等；`write_flush` 输出 `0000`；
   超长 data → `Err(Protocol)`；写完后数据已 flush（用一个 `Vec<u8>` 断言）。
4. **side-band 原样保留**：payload 以 `\x01`/`\x02`/`\x03` 开头时，`Pkt::Data`
   必须原样返回这些字节。

### (C) 防假绿
- 必须真的调 `minigit::transport::pktline` 的函数；不许用 `encode_pkt` 造 fixture
  再断言 `read_pkt` 能读（那只能证明自洽，不能证明与 git 一致）——
  所以**第 1 条必须用真实 git 输出**。
- 说明作者测试里哪些用例的真值来自 git。

## 3. 判定
明确 `PASS` / `FAIL`，逐条列「场景 → 命令 → 关键输出 → 是否成立」。

## 上报

- result 路径：`prompt.txt` 给出的绝对路径。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 不要贴完整逐字稿；给「命令 + 关键输出 + 结论」。**明确区分**
  「FAIL」「未覆盖（说明原因）」「已确认的已知限制」。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1 实测总结，必须遵守）

- **不要触发任何额外交互**：不启动引导/练习流程，不弹 Question/Ask 等人类输入。
  轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes, proceed / 2 don't ask again / 3 No」时选 1，
  **不要选 2**。
- **不要 export 会被 git 读取的环境变量**（`GIT_AUTHOR_*` / `GIT_COMMITTER_*`）。
  用 `git -c user.name=... -c user.email=...` 或 `env VAR=... git ...`。
- **共享 checkout**：本 wave 另有 3 个 agent 在改别的子目录。编译错误若出自作者白名单之外，
  记录并注明「不可归因于本任务」，等 30 秒重试，不要改别人的文件。
- **共享 `target/`**：并发 `cargo test` 会等文件锁（正常）。**变异测试必须用独立
  `CARGO_TARGET_DIR` 或独立副本**（W1 的 V2 因污染共享缓存返工过）。
