# T2 —— refs 层：HEAD、loose refs、packed-refs、原子更新与 CAS

你是本轮的**实现者**（kind: hermes）。仓库 `/home/user/Projects/mini-git` 是一个
「与真实 git 互操作」的教学实现（CLI 名 `mg`）。本轮只做 `src/refs/` 的实现。

## 1. 目标

把 `src/refs/store.rs` 里的全部 `todo(...)` 桩实现成真实逻辑（**签名不许改**）：

| 文件 | 需要实现的符号 |
|---|---|
| `src/refs/store.rs` | `RefStore::new` / `repo`（已有实现，别动）、`read_head`、`resolve`、`exists`、`list`、`packed`、`branches`、`update`、`delete`、`set_head_attached`、`set_head_detached` |

正确性标准：**与真实 git 的磁盘格式与命令输出一致**，不是「看起来合理」。
验收硬标准：`RefStore::list()` 的输出与同仓库上 `git show-ref` 的解析结果逐条相同；
`RefStore::branches()` 与 `git for-each-ref --format='%(refname:short)' refs/heads/` 相同。

## 2. 写作用域（白名单，只允许改这一个文件）

- `src/refs/store.rs`

可以在这个文件内部加 `#[cfg(test)] mod tests`（用 `tempfile` + 真实 `git` 二进制做对拍；
`tempfile` 已在 dev-dependencies 里，**不要动 Cargo.toml**）。**其余文件一律不许改**，特别是：
`src/refs/mod.rs`（CONTROLLER-OWNED，签名冻结）、`src/repo.rs`、`src/error.rs`、`src/oid.rs`、
`src/lib.rs`、`src/main.rs`、`src/cli/**`、`tests/**`、`.orch/**`、`Cargo.toml`、`Cargo.lock`。

如果你认为必须改白名单外的文件（例如需要新依赖、或发现 `mod.rs` 的签名不够用），
**不要改**：用 `blocked` 上报，写清楚「需要什么改动 + 为什么现有签名做不到」。

## 3. 关键格式与语义（照做，不要自己发明）

### 3.1 HEAD（`repo.head_path()` = `<git_dir>/HEAD`）

- attached：文件内容是 `ref: refs/heads/main\n` → `Head::Attached("refs/heads/main")`
  （**注意 `Head::Attached` 存的是含 `refs/heads/` 前缀的全名**，`branch_name()` 已经写好会去前缀）。
- detached：文件内容是 40 位 hex（可能带 `\n`）→ `Head::Detached(oid)`。
- 其它内容 → `Error::corrupt(...)`。文件不存在 → `Error::RefNotFound("HEAD")`。
- `set_head_attached(branch)` 参数是**短名或全名都要能处理**（`main` 与 `refs/heads/main` 都接受，
  落盘一律写全名）。写完必须能被真实 `git symbolic-ref HEAD` 正确读出。

### 3.2 loose ref

- 路径：`<git_dir>/<refname>`，例如 `refs/heads/main`。
- 内容：40 位小写 hex + `\n`。
- 创建前要 `create_dir_all` 父目录。

### 3.3 packed-refs（`<git_dir>/packed-refs`）

真实 git 的格式（注意这几条，写错就会被 git 判为损坏）：

```
# pack-refs with: peeled fully-peeled sorted
<oid> <refname>
^<peeled-oid>          # 紧跟在带注解 tag 行之后，是 tag 指向的 commit
```

- 第一行 comment 可选；有 comment 时行首是 `#`。
- `^` 行属于**上一条** ref 的 peeled 值；`RefStore::packed()` 只返回 `<oid>,<refname>` 对，
  不要把 `^` 行当成独立 ref（**读的时候必须跳过**，否则 `list()` 会多出垃圾条目）。
- `list()` = loose ∪ packed，**同名时 loose 覆盖 packed**，结果按 refname 字节序排序。
- `packed()` 只读 `packed-refs`，文件不存在返回空 Vec（不是错误）。

### 3.4 `resolve(name)` 要支持的形式（v1 子集）

按顺序尝试：`HEAD`；完整 40 位 hex（直接当 oid，**不要求对象存在**）；含 `/` 的全名（`refs/...`）；
短分支名（`refs/heads/<name>`）；tag 名（`refs/tags/<name>`）。都找不到 → `Error::RefNotFound(name)`。
**不要**实现 `~`/`^`/`@{...}`/短 oid 前缀/rev-parse 的完整语法，那些不属于本轮。

### 3.5 原子更新与 CAS（本轮最重要的部分）

- `update(name, new, expected)`：
  - `expected = None` → 要求 ref **不存在**，存在则 `Error::RefConflict`（这是「创建」语义）。
  - `expected = Some(None)` → 同样要求不存在。
  - `expected = Some(Some(oid))` → 要求当前值**恰好等于** oid，不等则 `Error::RefConflict{name, expected, actual}`。
  - 必须**真的原子**：写 `<ref>.lock`（用 `OpenOptions::new().write(true).create_new(true)`，
    已存在则说明有并发写者，报 `Error::RefConflict`），写完 `rename` 到目标路径。
    **不要**用「直接 `std::fs::write` 到目标路径」。
  - 更新分支时，若该 ref 只存在于 `packed-refs` 里，loose 写入后自动取得优先级（正确行为，无需改 packed-refs）。
- `delete(name)`：删 loose 文件（不存在则 `RefNotFound`）；若 `packed-refs` 里有同名条目，
  **必须同时把该条目从 packed-refs 重写掉**，否则删除会被 packed 值「复活」——这是真实 git 的行为，
  也是本轮最容易漏的坑。
- `exists(name)`：loose 或 packed 有任一即 true。

### 3.6 其它

- `Error` 变体见 `src/error.rs`：`RefNotFound(String)`、`RefConflict{name,expected,actual}`、
  `Corrupt{what,detail}`、`Other(String)`。用 `Error::corrupt(...)` 构造 Corrupt。
- 不要读/写 `reftable`、`.git/logs/**`（reflog）、worktree 的 `commondir`；遇到这些形态保持
  现有行为即可，不要为此扩展范围。

## 4. 验收（必须能独立复跑）

在 `src/refs/store.rs` 的 `#[cfg(test)] mod tests` 里写测试，并且**你自己先跑一遍**：

```bash
cd /home/user/Projects/mini-git
cargo test --offline refs::          # 必须全绿
cargo clippy --offline --all-targets # 必须 0 warning
scripts/check-freeze.sh              # 必须打印 drift 0
```

测试里必须至少有这几类（用 `tempfile::tempdir()` + 真实 `git` 对拍，**真值必须来自 git 二进制，
不能自己手写期望字符串**）：

1. **读真实 git 的仓库**：`git init` → `git commit` → `mg` 侧 `list()` 的条目集合/oid
   与 `git show-ref` 逐条相同；`branches()` 与 `git for-each-ref --format='%(refname:short)' refs/heads/` 相同。
2. **attached / detached HEAD**：`git symbolic-ref HEAD` 与 `read_head()` 一致；
   `set_head_detached` 后 `git symbolic-ref HEAD` 必须失败而 `git rev-parse HEAD` 等于该 oid。
3. **packed-refs**：跑一次 `git pack-refs --all`（或 `git gc`）后 loose 文件消失，
   `list()` 仍必须返回同样的 refs 集合（证明读 packed 生效），且 `packed()` 非空。
4. **CAS 反例（必须失败）**：用错误的 `expected` 调 `update` 必须返回 `Error::RefConflict`；
   ref 已存在时用 `expected=None` 创建必须冲突；**并且这两次都必须不改变磁盘上的 ref**。
5. **delete 反例/正例**：packed 之后 `delete("refs/heads/x")`，必须让 `git show-ref` 也看不到它。

## 5. 非目标

- 不要动 `Cargo.toml`；需要新依赖请报 `blocked`。
- 不支持 reftable、reflog、`git worktree` 的 `commondir`、非 UTF-8 refname、`~`/`^`/短 oid rev 语法。
- 不改任何 `pub` 签名；不新增 `pub` 项到模块外可见的范围（内部 `fn`/`struct` 私有实现随便加）。
- 不要顺手「修」`src/refs/mod.rs` 的注释或文档。

## 6. 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**，不要自己猜、不要写到别处。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`，见 agent-controlled 协议。
- `output` 里必须包含：跑了哪些命令 + 真实结论（例如「`cargo test --offline refs::` → 9 passed」）；
  不要贴完整逐字稿，也不要贴大段源码。

## 7. 深度与上限

- 本轮的 depth：1
- `max_depth`：3
- 若需要再往下委派：必须把 `--parent-depth 1` 显式写死，不得使用 shell 变量默认值。

## 8. 交互纪律（控制器实测总结，必须遵守）

- **不要触发任何额外交互**：不启动「学习练习 / tutorial / 引导流程 / 交互式向导」，
  不弹 Question/Ask 对话框等待人类输入。轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes / 2 don't ask again / 3 No」时选 1，
  **不要选 2**（不做常驻批量授权）。
- **共享 checkout 的编译噪声**：本 wave 另有 2 个 agent 同时改 `src/index/`、`src/worktree/`。
  若 `cargo` 报错的文件**不在你的白名单里**，那不是你的问题：等 30 秒重试；
  连续 3 次仍失败则不要改别人的文件，在 `output` 里记录并有界等待（必要时报 `blocked`）。
- **cargo target 目录是共享的**：并发 `cargo test` 会等文件锁，属正常现象，不是错误。
