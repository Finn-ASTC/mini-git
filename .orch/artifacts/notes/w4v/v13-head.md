# V13 —— 独立验证 T13（`file://` 传输 + clone/fetch/push/pull）

你是本轮的**验证者**（kind: omp），不是实现者。作者是 codex。
**不接受作者自述**，以真实 `git` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W4/T13-transport/task.md`（先读）
- 作者结果：**由 controller 在 prompt 里给出的 result 路径**（只读，不得修改）
- 作者可改文件：`src/transport/{local,negotiate}.rs`、`src/cli/{clone,fetch,push,pull}.rs`
- 开工前基线：`.orch/waves/W4/T13-transport/baseline.txt`（由 controller 在 wave 启动时拍）

## 1. 写作用域（白名单）

- `tests/verify_transport.rs`（**新建**）
- `.orch/waves/W4/T13-transport/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件：`src/**`（只读）、`Cargo.toml`、`Cargo.lock`、别人的 `tests/**`、
`.orch/rounds/**`。发现缺陷不要代改，写进结论（要 controller round）。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline transport::
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_transport.rs`，零硬编码真值）

1. **协议字节级**（不能只看「clone 成功」）：
   - 用**真实 git 服务端**产字节：`git upload-pack --advertise-refs <repo>`（本地直跑，
     `GIT_DIR`/`--stateless-rpc` 按需）把输出喂给 `parse_advertisement`，
     与 `git show-ref`/`git rev-parse` 的 oid 集合**完全相等**（含 `symref=HEAD:...`）；
   - `build_fetch_request` 的输出必须能被 `git upload-pack --stateless-rpc <repo>` 消费并回出
     合法 pack：断言 side-band 通道 1 拼出的字节 `git index-pack --stdin` 或
     `git unpack-objects -q` **exit 0**；
   - `build_push_update` 的输出喂 `git receive-pack --stateless-rpc <repo>`，
     `parse_report_status` 解出的 `ok/ng` 与真实服务端结论一致。
2. **双向互操作硬标准**（逐条给命令 + 原始输出）：
   - `mg clone file:///tmp/.../src dst` → `git -C dst fsck --no-progress` 无 error、
     `git -C dst log --oneline --all` 与源一致、`git -C dst status --porcelain` 为空；
   - **真实 git 能读 mg 写的仓库**：`git -C dst clone` 回来或 `git -C dst fsck` 通过；
   - `mg fetch` 后 `git show-ref` 的 `refs/remotes/origin/*` oid 与源一致；`FETCH_HEAD` 存在；
   - `mg push` 到 `file://` 远端后 `git -C remote log --oneline --all` 能看到新提交、`git fsck` 无 error；
   - **non-fast-forward**：先让远端前进，再 `mg push` → 非零退出且
     `git -C remote rev-parse <ref>` **与 push 前逐字节相同**；`--force` 时才变。
3. **反例**：不存在的 url、非仓库目录、空仓库（无 HEAD）、只读目录 → 清晰 `Err`、不 panic、
   **不留下半成品目录**（失败后 `ls -a` 检查，`mg clone` 失败不应留 `.git` 或目标目录残留）。
4. **不许假绿**：确认作者的实现真的走 pkt-line 编解码路径（读代码确认，并在 result 里指出
   你检查了哪个函数）；若发现作者用「本地 API 直接拷对象」冒充传输，直接判 FAIL。

### (C) 与 T14 的边界

`http://` 分支本轮由 T14 并行实现，不在你的判定范围。你只需确认 `file://` 分支没有把
`http://` 路径写死成错误行为（例如 url 路由是否明确），并在 result 里注明未覆盖。
