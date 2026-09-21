# T14 —— smart HTTP 传输（`http://`，本机 `git http-backend`）

你是本轮的**实现者**（kind: opencode）。仓库 `/home/user/Projects/mini-git` 是「与真实 git 互操作」
的实现（CLI 名 `mg`）。本轮只做一件事：**用明文 HTTP/1.1 与真实 `git http-backend` 通话**。

## 1. 目标（签名不许改）

| 文件 | 需要实现的符号 |
|---|---|
| `src/transport/http.rs` | `HttpRemote::new(url)`、`info_refs(&self, service) -> Result<Vec<u8>>`、`upload_pack(&self, body) -> Result<Vec<u8>>`、`receive_pack(&self, body) -> Result<Vec<u8>>` |

`src/transport/mod.rs` 是 **CONTROLLER-OWNED**（`pub` 声明区冻结），不要改。
`src/transport/negotiate.rs` 由 **T13（codex）本轮并行实现**：你**只能调用**它已冻结的四个函数
（`parse_advertisement` / `build_fetch_request` / `build_push_update` / `parse_report_status`），
**不要修改那个文件**；如果你需要新增 helper，写进 `http.rs` 私有函数，或在 result 里提出
「需要 controller round 扩展 negotiate.rs」。

## 2. 写作用域（白名单）

- `src/transport/http.rs`
- `tests/verify_http.rs`（**新建**，如果你需要在测试里起一个本地 HTTP 服务器也可以写在这里；
  也可以写 `tests/` 下的**新**文件，但不要动别人的 `tests/verify_*.rs`、`tests/interop/**`）

**不许改**：`src/transport/{mod,pktline,local,negotiate}.rs`、`src/odb/**`、`src/refs/**`、
`src/index/**`、`src/worktree/**`、`src/cli/**`（路由由 T13 做）、`Cargo.toml`、`Cargo.lock`、`.orch/**`。

## 3. 关键语义

1. **只用 `std::net::TcpStream` 实现明文 HTTP/1.1**（不允许引 HTTP/TLS 依赖）。
   目标端点（git 的 smart HTTP）：
   - `GET <url>/info/refs?service=git-upload-pack`（fetch）
   - `GET <url>/info/refs?service=git-receive-pack`（push）
   - `POST <url>/git-upload-pack` / `POST <url>/git-receive-pack`
2. **请求**必须带：`Content-Type: application/x-git-upload-pack-request`（或 receive-pack）、
   `Accept`、`Content-Length`、`User-Agent`（git 会看 `User-Agent` 决定协议版本，
   用 `git/2.x` 形态的字符串以便服务端走 v0）、`Host`；HTTP/1.1 需要 `Connection: close`
   或自己处理 keep-alive（简单起见用 close）。
3. **响应**要能处理 `Content-Length` 与 `chunked` 两种；`info/refs` 响应开头会有
   `# service=git-upload-pack\n` 服务行 + 一个 flush-pkt，必须**按 git 的规则跳过**再交给
   `pktline::read_pkt`（`upload_pack`/`receive_pack` 的响应是纯 pkt-line，不需要跳过服务行）。
4. `https://` → `Error::Unsupported`（v1 不引 TLS）。
5. 非 200 响应要带状态码与 body 前若干字节的错误信息（便于排障），不要静默返回空。

## 4. 验收（必须能独立复跑）

```bash
cd /home/user/Projects/mini-git
cargo test --offline --test verify_http
cargo test --offline
cargo clippy --offline --all-targets   # 0 warning
scripts/check-freeze.sh                # drift 0
```

1. **测试里起一个本地 HTTP 服务器**（写在你的测试文件里，用 `std::net::TcpListener` 即可），
   把请求转给真实 `git http-backend`（CGI：设置 `GIT_PROJECT_ROOT`、`PATH_INFO`、`QUERY_STRING`、
   `REQUEST_METHOD`、`CONTENT_TYPE`、`GIT_HTTP_EXPORT_ALL=1`，把 body 喂 stdin，读 stdout 里的
   header + body 回给客户端）。**这是测试脚手架，不是产品代码。**
2. 硬标准（必须逐条做）：
   - `info_refs("git-upload-pack")` 解出的 advertisement（用 `parse_advertisement`）
     与 `git upload-pack --advertise-refs` 的 ref 集合一致（含 `symref=HEAD:...`）；
   - 用 `build_fetch_request` 造请求 → `upload_pack` → 响应能被 `pktline` 逐帧解析，
     side-band 通道 1 拼出的 pack 交给 `git unpack-objects -q` **exit 0**；
   - `receive_pack` 至少验证「服务端返回 report-status 且能被 `parse_report_status` 解析」
     （push 的完整链路是 T13 的范围，你只保证 HTTP 层正确）；
   - `chunked` 响应分支必须有测试（让脚手架故意用 chunked 回一次）。
3. **反例**：404/500、缺 `Content-Length` 且非 chunked、连接被对端关闭、
   非 HTTP 响应（例如返回纯文本 "hello"）→ 必须是清晰 `Err`，不许 panic、不许死循环。
4. **不许假绿**：必须真的起服务端、真的走 TCP；不许 mock `TcpStream` 的功能来"模拟"协议，
   也不许把 `git` 客户端当成被测对象。

## 5. 非目标

- 不做 `https://`、不做 HTTP/2、不做代理/认证（`Authorization` 只在 result 里记为已知限制）、
  不做 protocol v2（若服务端仍回 v2，用 `User-Agent`/`Git-Protocol` 头控制成 v0 并说明）。
- 不改 `Cargo.toml`；需要新依赖请报 `blocked`。

## 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：命令 + 真实结论（哪几条硬标准成立、chunked 分支怎么测的、反例是否真的 Err）；
  已知限制/未覆盖项单列。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1/W2/W3 实测总结）

- 不触发任何额外交互；原生审批只按「单条」处理（选 1，不选 2）。
- 不 export `GIT_*` 环境变量（`git http-backend` 需要环境变量时，用 `Command::env` 就地传，
  不要 export 到整个 shell）。
- 共享 checkout：T13 正在并行改 `src/transport/{local,negotiate}.rs` 与 `src/cli/{clone,fetch,push,pull}.rs`，
  编译错误若出自这些文件，注明「不可归因于本任务」等 30 秒重试，**不要**去改。
- **端口**：测试里的监听端口用 `127.0.0.1:0`（系统分配）并打印实际端口，避免与其它 agent 抢占固定端口。

## 附：前提可被质疑（W3 教训，必读）

本任务书里的**任何**「真实 git 会 X」的断言都只是 controller 的提示，不保证完整：
W3/T11b 实测到一个反例 —— 同一句「写路径遇 symlink 祖先时 git 放行」在夹具不同（顺 symlink 能否
`stat` 到目标）时结论**相反**。因此：

- 动手前先自己用真实 git 复跑一遍任务书里的关键前提；若与实测不符，**先报冲突（附原始命令与输出），
  再按实测的真值实现**，不要为了「照做」而写出与 git 不一致的代码；
- 反过来，如果本任务书的**验收标准**与真实 git 冲突，以真实 git 为准，并在 result 里写明冲突点。

## 附：测试不得「skip 即通过」

任何测试用例都不允许在环境缺失（`git` 不在、网络不可达等）时 `return` 成**通过**：
要么 `panic!`/`assert!` 硬失败，要么用 `#[ignore]` 显式标出（W3/V9 在 T9 的作者用例里发现过这种假绿）。
