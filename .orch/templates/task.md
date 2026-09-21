# <任务号> <一句话标题>

> 复制本模板到 `.orch/waves/W<n>/<Tn>-<name>/task.md` 后填写。
> 六项都要有；缺任何一项，这一轮就测不出东西（§5）。

## 1. 目标

- 实现什么：<一句话>
- 公共 API（**已冻结，不要改签名**）：`<文件>:<符号>`
- 允许改的文件（写作用域，白名单）：
  - `<path>`
- 明确禁止改的文件：其余全部，尤其是 `Cargo.toml`、`src/lib.rs`、`src/main.rs`、
  各 `src/*/mod.rs` 的 `pub` 声明区、`tests/interop/**`（见 `.orch/FREEZE-v0.md`）。

## 2. 背景与接口

<格式/算法要点；指向代码里的 doc comment，不要让 agent 从零猜>

## 3. 验收（必须能独立复跑）

```bash
# 单元测试
cargo test --offline <module>::
# 与真实 git 的差分（这是硬标准）
<具体命令>
```

- 期望结果：<可判定的表述，例如「与 `git ls-files --stage` 输出逐字节相同」>
- 反例（必须失败的情形）：<例如「故意损坏 trailer 后必须报 Corrupt，而不是通过」>

## 4. 非目标

- <例如：不要动 Cargo.toml；需要新依赖请报 blocked>
- <例如：不支持 index v3/v4，读到就报 Unsupported>

## 5. 上报

- result 路径：`<绝对路径，由 protocol.py prepare 生成>`
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`，见 agent-controlled 协议。
- `output` 里必须包含：跑了哪些命令 + 真实结论；不要贴完整逐字稿。

## 6. 深度与上限

- 本轮的 depth：<收到的 depth>
- `max_depth`：<收到的上限，默认 3>
- 若需要再往下委派：必须把 `--parent-depth` 设为本轮 depth（不得使用 shell 变量默认值）。

## 7. 交互纪律（控制器实测总结，必须遵守）

- **不要触发任何额外交互**：不启动「学习练习 / tutorial / 引导流程 / 交互式向导」，
  不弹 Question/Ask 对话框等待人类输入。轮次结束的标志只有 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes / 2 don't ask again / 3 No」时选 1，
  **不要选 2**（不做常驻批量授权），也不要为了少弹窗而把命令合并成一条庞大脚本。
- **共享 checkout 的编译噪声**：本 wave 多个 agent 同时改不同子目录。
  若 `cargo` 报错的文件**不在你的写作用域内**，那不是你的问题：
  等 30 秒重试；连续 3 次仍失败则**不要改别人的文件**，在 result 的 `output` 里记录
  并继续做你能做的检查（必要时报 `blocked` 让别人先收敛）。
- **cargo target 目录是共享的**：并发 `cargo test` 会等文件锁，属正常现象，不是错误。
- **不要在持久 shell 里 export 会被 git 读取的环境变量**（`GIT_AUTHOR_NAME` / `GIT_AUTHOR_EMAIL` /
  `GIT_COMMITTER_*` / `GIT_CONFIG_*` 等）。这些会泄漏进你 shell-out 到真实 `git` 的子进程，
  让**别人的**测试出现假红/假绿（W1 的 V4 真实踩到过：`GIT_AUTHOR_NAME=a` 泄漏导致
  `tests/verify/main.rs` 报 `'a' vs 'A U Thor'`）。需要固定身份时用
  `git -c user.name=... -c user.email=...` 逐条传参，或 `env VAR=... git ...` 限定作用域。
