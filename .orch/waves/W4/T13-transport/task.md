# T13 —— `file://` 传输 + clone/fetch/push/pull

你是本轮的**实现者**（kind: codex）。仓库 `/home/user/Projects/mini-git` 是「与真实 git 互操作」
的实现（CLI 名 `mg`）。本轮把**远端协作**打通：`file://` 传输 + 四条远端命令。

## 1. 目标（签名不许改）

| 文件 | 需要实现的符号 |
|---|---|
| `src/transport/negotiate.rs` | `parse_advertisement`、`build_fetch_request`、`build_push_update`、`parse_report_status` |
| `src/transport/local.rs` | `fetch`、`push`、`clone_into` |
| `src/cli/clone.rs` | `run(url: &str, dir: Option<&Path>) -> Result<()>` |
| `src/cli/fetch.rs` | `run(remote: Option<&str>, refspec: Option<&str>) -> Result<()>` |
| `src/cli/push.rs` | `run(remote, refspec, set_upstream, force) -> Result<()>` |
| `src/cli/pull.rs` | `run(remote: Option<&str>) -> Result<()>` |

`src/transport/mod.rs`、`src/cli/mod.rs` 是 **CONTROLLER-OWNED**：`pub` 声明区冻结，
`FetchOutcome` / `RefAdvertisement` / `FetchRequest` / `PushCommand` 的字段不许改。
`src/transport/pktline.rs`（T8 已完成）**只读复用**，不要改。

## 2. 写作用域（白名单，只有这六个文件）

- `src/transport/{local,negotiate}.rs`
- `src/cli/{clone,fetch,push,pull}.rs`

**不许改**：`src/transport/{mod,pktline,http}.rs`、`src/odb/**`、`src/refs/**`、`src/index/**`、
`src/worktree/**`、`src/merge/**`、`src/cli/mod.rs`、其它 `src/cli/*.rs`、`Cargo.toml`、
`Cargo.lock`、别人的 `tests/**`、`.orch/**`。需要改 → 报 `blocked`。

## 3. 关键语义（按真实 git 的协议 v0）

1. **`file://` 必须走 pkt-line 编解码路径**：把对端仓库当 `Repo` 打开（本地直读），
   但**产出的字节流要与真实服务端一致**——即自己按 v0 语义生成 ref advertisement
   与 pack 流，再用 `pktline::{read_pkt,write_pkt,write_flush}` 编解码。
   不要直接调本地 API 把对象拷过去（那样 T14 的 HTTP 就没法复用你的协商逻辑）。
2. **fetch**：
   - 读 advertisement（第一行 `<oid> <name>\0<caps>`，取 `side-band-64k`、`symref=HEAD:...`）；
   - 发 `want <oid> <caps>\n` × n + flush + `have <oid>\n` × n + `done\n`；
   - 收 `NAK`/`ACK`，然后 `side-band` 通道 1 里是 pack 数据（通道 2 是进度，要丢弃/转发到 stderr）；
   - **把 pack 解出来写成 loose 对象**（复用 T9 的 `PackFile::read_at` + `PackSet`；
     注意 thin pack 的 `REF_DELTA` base 可能在本地已有对象里 —— 这是 fetch 与 `git gc` 产出的
     自包含 pack 的关键区别，必须处理，或明确报 `Unsupported` 并在 result 里说清）；
   - 更新 `refs/remotes/<remote>/<branch>`（默认 remote 名 `origin`），写 `FETCH_HEAD`。
3. **push**：
   - 先读 advertisement（`report-status` capability）；
   - 计算需要发送的对象（可达闭包），生成 pack（**可以自己写 pack 的编码**，
     但不要新增 `src/odb/**` 的公共 API —— 需要就写在 `local.rs` 私有函数里）；
   - 发 `<old> <new> <ref>\0<caps>` + flush + pack 数据，收 `report-status`；
   - **non-fast-forward 必须拒绝**（除非 `--force`），退出码非 0 且**远端一字未改**；
   - `-u/--set-upstream` 写 `branch.<name>.remote` / `branch.<name>.merge`（可用 `Config` 写文件？
     若 `Repo`/`Config` 没有写接口，就写进 result 的「已知限制」——**不要**改 `src/repo.rs`）。
4. **clone**：`clone_into(url, dir)` = 建新仓库 → fetch → 检出 HEAD（复用 `Worktree::materialize_tree`
   + `RefStore`）→ 写 `refs/remotes/origin/*` 与 remote 配置（能写就写）。
   `mg clone <url> [dir]` 后：`git fsck` 通过、`git log --oneline` 与源仓库一致、`git status` 干净。
5. **pull**：fetch 后合并（fast-forward 优先，非 ff 用 `merge::merge_base` 判断；冲突走 T10 的
   `three_way`）。若 T10 本轮尚未合入，pull 的冲突分支可报 `NotImplemented` 并写进「已知限制」。
6. **URL 路由**：`file://` 走 `local.rs`；`http://` 调 `HttpRemote::new(url)`（T14 并行实现，
   签名已冻结）；`https://` 报 `Unsupported`（v1 不引 TLS）。**只实现 file:// 分支**。

## 4. 验收（必须能独立复跑）

```bash
cd /home/user/Projects/mini-git
cargo test --offline transport::
cargo test --offline
cargo clippy --offline --all-targets   # 0 warning
scripts/check-freeze.sh                # drift 0
```

1. **与真实 git 双向互操作（硬标准）**：
   - `mg clone file:///tmp/src dst` 后：`git -C dst fsck --no-progress` 无 error、
     `git -C dst log --oneline` 与源一致、`git -C dst status --porcelain` 为空；
   - 反向：`git clone file:///tmp/src dst2` 之后用 `git -C dst2 push` 推不过去没关系，
     关键是**真实 git 能读 mg 写的仓库**；
   - `mg fetch` 后 `git show-ref` 能看到正确的远端引用 oid；
   - `mg push` 到 `file://` 远端后，**真实 `git -C remote log --oneline`** 能看到新提交，
     且 `git fsck` 无 error；
   - non-fast-forward：先让远端前进，再 `mg push` → 非零退出且远端 `git rev-parse` **未变**；
     `--force` 时才改。
2. **协议层**：`parse_advertisement` / `build_fetch_request` / `build_push_update` /
   `parse_report_status` 都要单测；真值来自**真实 git 进程**产出的字节
   （例如用一个真实仓库做 `git upload-pack --advertise-refs` 的输出来喂 `parse_advertisement`）。
3. **反例**：不存在的 url、不是仓库的目录、空仓库（无 HEAD）、权限不足 →
   清晰错误、不 panic、不留下半成品目录（失败后 `ls` 检查）。
4. **不许假绿**：测试必须真的调 `mg` 二进制与 `minigit::transport::local`；
   不许把对象拷贝当成「走了传输协议」——至少要有一处**字节级**的 pkt-line 断言。

## 5. 非目标

- 不做 v2 协议、不做 shallow/partial clone、不做 `git://`（TCP 原生协议）、
  不做 `https://`、不做 push-cert、不做 submodule、不做 `--depth`。
- 不改 `Cargo.toml`（不能引 HTTP/TLS/网络依赖；`file://` 不需要）。
- 不新增 `src/odb/**` 的公共 API。

## 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：命令 + 真实结论（clone/fetch/push 的 fsck 结果、non-FF 是否真的没改远端、
  字节级协议断言在哪）；已知限制/未覆盖项单列。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1/W2/W3 实测总结）

- 不触发任何额外交互；原生审批只按「单条」处理（选 1，不选 2）。
- 不 export `GIT_*`（用 `git -c key=value` / `env VAR=... git ...`）。
- 共享 checkout：编译错误若出自你的白名单之外，注明「不可归因于本任务」等 30 秒重试。
- 变异测试/临时构建用独立 `CARGO_TARGET_DIR` 或独立副本；临时仓库放 `tempfile::tempdir()` 或 `/tmp/<你的名字>/`。

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
