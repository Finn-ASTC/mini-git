# T4 —— worktree 层：扫描、.gitignore 匹配、三方 status

你是本轮的**实现者**（kind: codex）。仓库 `/home/user/Projects/mini-git` 是一个
「与真实 git 互操作」的教学实现（CLI 名 `mg`）。本轮只做工作区扫描 / ignore / status 三块。

## 1. 目标

| 文件 | 需要实现的符号 |
|---|---|
| `src/worktree/scan.rs` | `scan_worktree(repo) -> Result<Vec<WorktreeFile>>` |
| `src/worktree/ignore.rs` | `Ignore::load(repo)`、`Ignore::is_ignored(path, is_dir)` |
| `src/worktree/status.rs` | `StatusLine::xy`、`StatusReport::porcelain`、`Worktree::status`、`Worktree::read_worktree_entry` |

**本轮不做 `src/worktree/materialize.rs`**：那一块（tree → 工作区写出）依赖还没实现的 odb
（W2 的 T5），已排到 W3 的 T11。**它是冻结桩，不许改**（改了也没有测试能验证，且会越界）。

正确性标准（硬标准，可判定）：`StatusReport::porcelain()` 的输出与同仓库上真实
`git status --porcelain` **逐字节相同**。

## 2. 写作用域（白名单，只允许改这三个文件）

- `src/worktree/scan.rs`
- `src/worktree/ignore.rs`
- `src/worktree/status.rs`

可以在这三个文件内部加 `#[cfg(test)] mod tests`（用 `tempfile` + 真实 `git` 二进制做对拍；
`tempfile` 已在 dev-dependencies 里，**不要动 Cargo.toml**）。**其余文件一律不许改**，特别是：
`src/worktree/mod.rs` 与 `src/worktree/materialize.rs`（CONTROLLER-OWNED / 冻结桩）、
`src/index/**`（另一个 agent 本轮在改）、`src/refs/**`（第三个 agent 本轮在改）、
`src/repo.rs`、`src/error.rs`、`src/oid.rs`、`src/lib.rs`、`src/main.rs`、`src/cli/**`、
`tests/**`、`.orch/**`、`Cargo.toml`、`Cargo.lock`。

如果你认为必须改白名单外的文件（例如需要新依赖、或 `mod.rs` 的签名/语义不够用），
**不要改**：用 `blocked` 上报。

## 3. 控制器澄清的接口语义（重要，写代码前先读）

`Worktree::status(&self, head_tree: Option<&Tree>, index: &Index)` 里的 `head_tree` 是
**已展平的** tree：即 `Tree::entries()` 里每一项的 `name` 都是**仓库相对全路径**
（例如 `a/b/c.txt`），而不是单层名字。调用方（`cli/status.rs`，W3 的 T12）负责
递归读子树并展平；本轮你不需要读 odb（`src/odb/**` 还没实现）。
- 因此：**不要**尝试用 `Tree::lookup` 逐层下降，也不要读 `.git/objects`。
- 防御性处理：遇到 `FileMode::Tree` 的条目直接忽略。
- `head_tree = None` 表示仓库还没有提交（HEAD 不存在），此时 X 列全部为空。

`Index` 相关类型（`src/index/mod.rs`）**已经实现好且冻结**，直接用；`Index::entries` 是
按 `(path, stage)` 排好序的。**不要**调用 `Index::read` / `Index::write`（那是 T3 的文件，
本轮可能还没实现完）——你要在测试里自己构造 `Index`。

## 4. 关键语义（照做，不要自己发明）

### 4.1 `scan_worktree`

- 不进入 `.git`（以及 `.git` 的任何形态）。**空目录不产出条目**（git 不跟踪目录）。
- 路径是仓库相对、`/` 分隔的 UTF-8 字节。
- symlink **不跟随**：`is_dir = false`（用 `symlink_metadata`）。
- 整个目录被 ignore 时**不要下降**（git 依赖这个行为来折叠 `dir/` 形式的未跟踪目录）：
  此时产出该目录自身一条（`is_dir = true`, `ignored = true`）即可。
- `ignored` 字段：用 `Ignore` 判定（`is_dir` 影响 `dir_only` 规则）。
- 返回顺序按 path 字节序排序（status 依赖它，省得再排一次）。

### 4.2 `Ignore`

- 读**每个目录下**的 `.gitignore`（根目录 + 各级子目录），规则只作用于该目录及其子目录
  （即 `IgnoreRule::base` 存储规则所在目录的仓库相对路径，根目录是空 `Vec`）。
- 支持：`*` `?` `[...]`、`!` 取反、前导 `/` 锚定（相对 `base`）、尾随 `/` 只匹配目录、
  以 `#` 开头的注释行、空行、行尾未转义空格的处理（简单处理即可）。
- **最后一条匹配的规则胜出**（因此 `!` 可以救回被忽略的文件）。
- 目录规则要能作用于其子路径：`build/` 忽略后 `build/x/y.txt` 也算被忽略。
- 不支持（**不要实现**）：`.git/info/exclude`、global excludes、`core.excludesFile`、
  `**` 的完整语义（按普通 `*` 处理即可，并在代码注释里说明）。

### 4.3 `StatusLine::xy`

返回两个字符：
- 冲突时返回 `ConflictKind::porcelain_code()`（`UU`/`AA`/`AU`/`UA`/`DU`/`UD`），其余列忽略。
- 未跟踪：`"??"`。
- 否则 X 列 = `index`（`Option<ChangeKind>` → `code()`，`None` 是空格），
  Y 列 = `worktree`（同上）。例如 `" M"`、`"M "`、`"MM"`、`"D "`、`"A "`。

### 4.4 `StatusReport::porcelain`

`git status --porcelain` v1：每行 `XY SP <path> LF`，**末尾有换行**；行按 path 字节序。
未跟踪目录折叠成一条 `?? dir/`（见 4.5）。

### 4.5 `Worktree::status` —— 三方比对（本轮核心）

| 比较 | 列 | 取值 |
|---|---|---|
| HEAD（展平 tree） vs index（stage 0）| `X` | `A`（index 有、HEAD 没有）、`M`（都有但 oid 或 mode 不同）、`D`（HEAD 有、index 没有）|
| index（stage 0） vs 工作区 | `Y` | `M`、`D`（文件不存在）|
| 只有在工作区、且不在 index、且不被 ignore | — | `??` |
| index 里 stage 1/2/3 | — | `UU`/`AA`/`AU`/`UA`/`DU`/`UD`（见 `ConflictKind`）|

细则：
- **内容判定优先正确性**：`Y` 列直接读工作区内容算 blob oid（`Oid::hash_object("blob", bytes)`）
  与 index 里的 oid 比；mode 不同（普通 ↔ 可执行、普通 ↔ symlink）也算 `M`。
  **不要**只信 stat（`stat_matches` 只在你想做性能优化时用，正确性优先）。
- `index` 里的路径若在工作区是**目录**或不存在 → `D`。
- **未跟踪目录折叠**：如果某个目录下的**所有**文件都不在 index 里，则只报一条 `?? <dir>/`
  （这就是 `scan_worktree` 不下降 ignore 目录的原因）。目录里有任何已跟踪文件时不折叠。
- 空目录不出现。
- `porcelain()` 不包含 `.gitignore` 里被忽略的文件。

## 5. 验收（必须能独立复跑）

```bash
cd /home/user/Projects/mini-git
cargo test --offline worktree::     # 必须全绿
cargo clippy --offline --all-targets
scripts/check-freeze.sh             # drift 0
```

测试写法（真值全部来自真实 `git` 二进制）：

1. 用 `git init` 建仓库并造出多样状态（已提交文件被改 / 被删 / 新增并 `git add` /
   只在工作区的新文件 / 未跟踪目录 / `.gitignore` 覆盖的文件 / 可执行位变化 / symlink）。
2. 用 `git status --porcelain` 拿真值（**默认参数**，不要加 `-unormal`/`-uall`）。
3. 用 `git ls-tree -r -z HEAD` 解析出**展平**的 `Tree`；用 `git ls-files --stage -z`
   解析出 `Index`（注意 stage 字段与 mode 字符串；oid 用 `Oid::from_hex`）。
4. 调 `Worktree::status(...)`，断言 `report.porcelain() == git status --porcelain` 的原始输出
   （逐字节，含末尾换行）。
5. **无提交的仓库**：`head_tree = None` 也必须与 git 一致。

反例（必须失败/或明确不支持）：
- 截断/损坏的 `.gitignore` 不应 panic；
- `is_ignored("build/x", false)` 在只有 `build/` 规则时必须为 true；
- `!important.txt` 在 `*.txt` 之后必须救回该文件（最后一条胜出）。

已知限制（写在代码注释和 `output` 里即可，不用实现）：mg 不做 rename 检测，
所以「把 a 改成 b」在 mg 里是 `D a` + `A b`，而 `git status` 可能输出 `R  a -> b`。
测试场景里**不要构造 rename**；若必须构造，用 `git -c status.renames=false status --porcelain` 取真值。

## 6. 非目标

- 不实现 `materialize.rs`（W3/T11）。
- 不支持 `core.excludesFile`、`info/exclude`、skip-worktree/assume-valid 的语义差别、
  rename 检测、submodule、sparse checkout。
- 不要动 `Cargo.toml`；需要新依赖请报 `blocked`。
- 不改任何 `pub` 签名。

## 7. 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**，不要自己猜。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 必须包含：跑了哪些命令 + 真实结论（例如「porcelain 在 6 个场景下与 git 逐字节相同」）、
  以及你明确**没有**实现的限制。不要贴完整逐字稿。

## 8. 深度与上限

- 本轮的 depth：1
- `max_depth`：3
- 若需要再往下委派：必须把 `--parent-depth 1` 显式写死，不得使用 shell 变量默认值。

## 9. 交互纪律（控制器实测总结，必须遵守）

- **不要触发任何额外交互**：不启动「学习练习 / tutorial / 引导流程 / 交互式向导」，
  不弹 Question/Ask 对话框等待人类输入。轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes / 2 don't ask again / 3 No」时选 1，不要选 2。
- **共享 checkout 的编译噪声**：本 wave 另有 2 个 agent 同时改 `src/refs/`、`src/index/`。
  若 `cargo` 报错的文件**不在你的白名单里**，那不是你的问题：等 30 秒重试；
  连续 3 次仍失败则不要改别人的文件，在 `output` 里记录并有界等待（必要时报 `blocked`）。
- **cargo target 目录是共享的**：并发 `cargo test` 会等文件锁，属正常现象，不是错误。
