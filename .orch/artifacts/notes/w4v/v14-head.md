# V14 —— 独立验证 T14（smart HTTP + 本机 `git http-backend`）

你是本轮的**验证者**（kind: codex），不是实现者。作者是 opencode。
**不接受作者自述**，以真实 `git http-backend` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W4/T14-http/task.md`（先读）
- 作者结果：**由 controller 在 prompt 里给出的 result 路径**（只读，不得修改）
- 作者可改文件：`src/transport/http.rs`、`tests/verify_http.rs`
- 开工前基线：`.orch/waves/W4/T14-http/baseline.txt`（由 controller 在 wave 启动时拍）

## 1. 写作用域（白名单）

- `tests/verify_http_http.rs`（**新建**，如与作者文件名冲突就用这个名）
- `.orch/waves/W4/T14-http/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件，尤其 `src/**`（只读）、作者的 `tests/verify_http.rs`（只读）、
`Cargo.toml`、`Cargo.lock`、`.orch/rounds/**`。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline --test verify_http_http
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_http_http.rs`，零硬编码真值）

**你必须自己写一套 HTTP 脚手架**（`std::net::TcpListener` + `git http-backend` CGI），
**不要复用作者测试里的脚手架**：复用就等于用作者的假设验证作者。

1. **请求形状**：抓取 `HttpRemote` 实际发出的字节（在脚手架端记录原始 request line + headers），
   断言：正确的 `GET /info/refs?service=...`、`POST /git-upload-pack`、`Content-Type`、
   `Content-Length` 与 body 长度一致、`Host`、`User-Agent` 形态（能诱导服务端走 v0）。
2. **info/refs**：`# service=git-upload-pack\n` 服务行 + flush-pkt 被正确跳过；
   `parse_advertisement` 解出的 ref 集合与 `git upload-pack --advertise-refs` 一致。
3. **真实上传链路**：`build_fetch_request` → `upload_pack` → 响应逐帧 `pktline` 解析 →
   side-band 通道 1 拼出的 pack → `git unpack-objects -q` **exit 0**。
4. **chunked 分支**：让脚手架（或中间转发层）用 `Transfer-Encoding: chunked` 回一次，
   断言与 `Content-Length` 分支得到**逐字节相同**的 body；再测分块边界跨 TCP 包的场景
   （故意在 1 字节处切开 write + flush）。
5. **反例**：404/500、缺 `Content-Length` 且非 chunked、连接被对端提前关闭、
   返回纯文本 `hello` → 必须是清晰 `Err`，不许 panic、不许死循环（用超时线程守住）。
6. **https://** → `Error::Unsupported`，且有测试。

### (C) 不许假绿

- 必须真的起 `TcpListener`、真的走 TCP、真的起 `git http-backend` 进程；
  用 mock `TcpStream` 模拟协议直接判 FAIL。
- 检查作者是否把「git 客户端成功」当成「HTTP 层正确」——你要证明的是**被测的 HTTP 层**，
  不是 git 本身。
