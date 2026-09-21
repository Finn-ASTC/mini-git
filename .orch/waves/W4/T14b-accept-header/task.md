# T14b —— 修 `Accept` 头（V14 发现的低严重度缺陷）

你是 T14 的**同一个作者**（kind: opencode），本轮是返工轮。

## 0. 缺陷（V14 独立验证时发现，controller 复核）

`src/transport/http.rs:59` 的
```rust
format!("application/x-git-{service}-advertisement")
```
中 `service` 本身已是 `git-upload-pack`，于是发出 `Accept: application/x-git-git-upload-pack-advertisement`
（多一层 `git-`）。

- 真实 git 2.55.0 对 `GET <url>/info/refs?service=git-upload-pack` 发的是 **`Accept: */*`**；
- 规范 advertisement mime 是 `application/x-git-upload-pack-advertisement`（任一都行，但**不能**是现在这个）；
- POST 侧的 `Accept: application/x-git-upload-pack-result` 实测与真实 git 完全一致，**不要动**。

## 1. 目标

把 GET `info/refs` 的 `Accept` 改成与真实 git 逐字节一致（`*/*`），receive-pack 侧同理；
如果选择规范化 mime（`application/x-{service}-advertisement`），必须在 result 里说明为什么与真实 git 不同。

## 2. 写作用域（白名单）

- `src/transport/http.rs`
- `tests/verify_http.rs`（你自己的文件，可加断言）
- **不许改**：`src/transport/{mod,negotiate,local,pktline}.rs`、`src/cli/**`、`tests/verify_http_http.rs`
  （V14 的文件）、其它 `tests/**`、`Cargo.toml`、`Cargo.lock`、`.orch/**`。

## 3. 验收

```bash
cargo test --offline --test verify_http
cargo test --offline --test verify_http_http   # V14 的，必须仍然全绿
cargo test --offline
cargo clippy --offline --all-targets            # 你的文件 0 warning
scripts/check-freeze.sh
```
外加：**用真实 git 客户端做一次对拍**（`git -c protocol.version=0 ls-remote <你的脚手架 url>`），
把你发出的原始请求行与真实 git 的原始请求行并排贴进 result（method/target/Host/User-Agent/Accept/Content-Type）。

## 4. 上报

- result 路径：由 prompt.txt 给出（**新 round**）。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。

## 5. 非目标

- 只改这个头部；不要顺手重构 http.rs 的其它部分，不要改协商/传输逻辑。
- 不改测试脚手架的行为（V14 的用例要对同一脚手架继续通过）。

## 附：前提可被质疑 / 测试不得 skip 即通过 / 交互纪律

- 任务书里的「真实 git 会 X」只是提示，先自己复测；不符就先报冲突。
- 环境缺失时不许 `return` 成通过；要么硬失败，要么 `#[ignore]`。
- 原生审批只按「单条」处理（选 1，不选 2）；不 export `GIT_*`；临时仓库放 `/tmp/<你的名字>/`。
