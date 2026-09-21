# T12 —— 日常命令：add / rm / status / commit / log / tag

你是本轮的**实现者**（kind: hermes）。仓库 `/home/user/Projects/mini-git` 是「与真实 git 互操作」
的实现（CLI 名 `mg`）。本轮把「能日常用」的六个命令补齐，并顺带修掉 `mg status` 的一个已知缺口。

## 1. 目标（签名不许改）

| 文件 | 需要实现的符号 |
|---|---|
| `src/cli/add.rs` | `run(pathspec: &[PathBuf], verbose: bool) -> Result<()>` |
| `src/cli/rm.rs` | `run(pathspec: &[PathBuf], cached: bool, force: bool) -> Result<()>` |
| `src/cli/status.rs` | `run(porcelain: Option<&str>, short: bool) -> Result<()>` |
| `src/cli/commit.rs` | `run(message: Option<&str>, all: bool, amend: bool, allow_empty: bool, author: Option<&str>) -> Result<()>` |
| `src/cli/log.rs` | `run(oneline: bool, max_count: Option<usize>, rev: Option<&str>) -> Result<()>` |
| `src/cli/tag.rs` | `run(annotate: bool, message: Option<&str>, list: bool, name: Option<&str>, rev: Option<&str>) -> Result<()>` |
| `src/worktree/status.rs` | 让 `ChangeKind::TypeChanged`（W2 新增，`code()` → `'T'`）**真正被产出** |

> 注意 `src/cli/status.rs` 文件头的 doc 注释写的是「T4（codex）」，那是 W1 的过期标注，
> 以本任务书为准（文件本身不在冻结清单里，你可以顺手把注释改对）。

## 2. 写作用域（白名单，只有这七个文件）

- `src/cli/{add,rm,status,commit,log,tag}.rs`
- `src/worktree/status.rs`（**只允许**加「类型变化判定」相关逻辑；不要动它的其它语义）

**不许改**任何其它文件：`src/cli/mod.rs`、`src/worktree/mod.rs`、`src/worktree/materialize.rs`
（T11 本轮在改）、`src/index/**`、`src/refs/**`、`src/odb/**`、`src/object/**`、
`src/diff/**`、`src/merge/**`、`Cargo.toml`、`Cargo.lock`、别人的 `tests/**`、`.orch/**`。
需要改白名单外文件 → 报 `blocked`（不要越界改，W1 的 C-10 就是这么被发现的）。

## 3. 关键语义（按真实 git 对齐）

### 3.1 `mg add`
1. pathspec 展开：`.`、目录（递归）、文件、多参数；**跳过 ignored**（`Worktree::ignore()` 现成）。
2. **删除检测**：pathspec 覆盖范围内、index 里有但工作区已不存在的路径 → 从 index 删除条目。
3. 每个文件经 `Odb::write` 落 blob（幂等）→ `Index::upsert(IndexEntry::new(...))`，
   并把 `stat` 填成 `StatData::from_metadata`（racy-git 的 mtime 处理按 git 规则：
   **mtime 等于 index 写入秒时 size 必须为 0** 才能被信任；不确定就查 `IndexEntry::stat_matches` 的用法）。
4. 目录被整体删除时要删掉 index 里该前缀下的所有条目。
5. 硬标准：`mg add <paths>` 之后 `git ls-files --stage` 与真实 `git add` 的结果**完全一致**；
   `mg add .` 与 `git add .` 一致（含 ignore 行为）。

### 3.2 `mg rm`
- 默认：删 index 条目 + 删工作区文件；`--cached`：只删 index 条目。
- 当文件与 HEAD 或 index 不一致且没有 `-f` 时必须拒绝（`Error::WouldLoseChanges`），
  **且不得改动任何文件**。`git ls-files --stage` 与 `git status --porcelain` 对拍。
- `--cached` 删掉全部 stage 条目（用 `Index::remove_all_stages`）。

### 3.3 `mg status`
- `--porcelain`（v1）必须与 `git status --porcelain` **逐字节一致**：
  含 XY 码、`??`、重命名（v1 只在 staged rename 时输出 `R `，需要至少有一次 mv 检测？
  —— **如果实现 rename 检测成本过高，就明确写进「已知限制」**，但 `??`/`M`/`A`/`D`/`T`
  与**分组顺序**（先已跟踪变更、再未跟踪，组内按 path 排序）必须正确）；
  路径含空格/非 ASCII 时的引号与转义规则必须与 git 一致（C 风格引号 + 八进制转义）。
- `--short` 与 porcelain 同形（git 里二者等价）。
- **`T`（类型变化）**：index 里是普通文件、工作区是 symlink（或反之）、或工作区变成目录时，
  必须输出 `T` 而不是 `M`。这是本轮的**必做项**（W1 遗留缺陷，controller 已在
  `FREEZE-v0.md` v0.4 里加了 `ChangeKind::TypeChanged` 变体）。
- `--porcelain=v2` → 报 `NotImplemented`（写进已知限制，不要输出半成品）。

### 3.4 `mg commit`
1. index → **递归建 tree**（子目录先建，条目按 git 的排序规则：tree 条目名按字节序，
   目录名比较时视作带 `/`）。
2. parent = HEAD（unborn 时无 parent）；`.git/MERGE_HEAD` 存在时必须把它的 oid 作为**第二 parent**
   并在提交后删除 `MERGE_HEAD`/`MERGE_MSG`（这样 `mg commit` 能收尾 `mg merge`/真实 `git merge`
   留下的冲突状态）。
3. 无改动且没有 `--allow-empty` → 拒绝（git 的提示是 "nothing to commit"）；
   有未解决的冲突（index 里有 stage 1/2/3）→ 拒绝。
4. 写 commit 对象（author/committer 取 `user.name`/`user.email`，
   缺省要有可解释的兜底；`--author` 覆盖 author 但 committer 不变；
   **时区与时间格式必须与 git 一致**：`<epoch> +0800`）。
5. `RefStore::update` 用 CAS 更新分支引用（detached HEAD 时用 `set_head_detached`）；
   `--amend` 替换 HEAD（保留 HEAD~1 的 parent 链、**丢弃**原 HEAD 的 parent 重写）。
6. `-a`：先把已跟踪文件的改动写进 index（等价 `add -u`），再提交。
7. 硬标准：`mg commit -m x` 后 `git fsck --no-progress` 无 error、
   `git log --format=raw` 与期望一致、`git log -1 --format=%T` 等于 mg 写的 tree；
   **真实 `git commit` 能在 mg 留下的 index 上直接工作**（state 兼容）。

### 3.5 `mg log`
- 默认按提交时间优先（`--date-order`）、首父优先；`--oneline` = 短 oid(7) + summary
  （summary 取 message 第一行，超长按 git 规则处理）；`-n/--max-count`；
  `rev` 支持 `RefStore::resolve` 的写法。
- 硬标准：`mg log --oneline` 与 `git log --oneline` **逐字节一致**（含合并提交顺序），
  用至少 3 个含合并/分叉的仓库对拍。

### 3.6 `mg tag`
- 轻量：写 `refs/tags/<name>`；`-a -m`：写 tag 对象（tagger 行格式与 git 一致）再指向它；
  `-l`：列 tag（与 `git tag -l` 一致）；`-l -n`？不在签名内，跳过；
  删除不在签名内，跳过（写进已知限制）。
- 硬标准：`git tag -l`、`git cat-file -p refs/tags/<name>`、`git for-each-ref` 的
  objecttype 与 mg 的结果一致；`git fsck` 无 error。

## 4. 验收（必须能独立复跑）

```bash
cd /home/user/Projects/mini-git
cargo test --offline
cargo clippy --offline --all-targets   # 0 warning
scripts/check-freeze.sh                # drift 0
```

1. **每个命令都要与真实 git 对拍**：至少 3 个「有分叉、有合并、有 rename、有二进制文件、
   有非 ASCII 路径」的仓库；比较对象是 `git` 的**进程输出**与 `mg` 的**进程输出**
   （逐字节），以及 git 自己产出的对象（`git fsck`、`git cat-file`、`git log`）。
2. **互相收尾**：mg 写的仓库能被真实 `git status`/`git commit`/`git log`/`git tag` 直接使用；
   真实 git 写的仓库能被 mg 读。两条方向都要有测试。
3. **`T` 类型变化**：用真实 git 造出 `T` 场景（`git rm --cached f && ln -s x f` 之类），
   断言 `mg status --porcelain` 与 `git status --porcelain` 逐字节一致。
4. **反例**：`mg rm` 无 `-f` 且内容不一致 → 拒绝且**零文件改动**；
   冲突状态下 `mg commit` → 拒绝；`mg add` 一个不存在路径 → 非零退出 + stderr、不 panic。
5. **不许假绿**：测试必须真的调 `mg` 二进制（`env!("CARGO_BIN_EXE_mg")`）与
   `minigit::{index,refs,worktree,odb}`；真值来自真实 git 进程，不硬编码期望字节。

## 5. 非目标

- 不做 `--porcelain=v2`、不做 rename 检测的完整矩阵（做不到就写「已知限制」，
  不要写假数据）、不做 `git add -p`、不做 `commit --fixup`、不做 `log --graph`、
  不做 `tag -d`、不做 GPG 签名、不做 hooks。
- 不改 index 的 TREE/cache-tree 扩展（保留既有扩展，别破坏无损往返）。
- 不改 `Cargo.toml`；需要新依赖请报 `blocked`。

## 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**，不要自己猜、不要写到别处。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`（见 agent-controlled 协议）。
- `output` 必须包含：跑了哪些命令 + **真实结论**（逐字节是否一致、fsck 结果、
  拒绝场景是否零改动）；不要贴完整逐字稿，也不要贴大段源码。已知限制/未覆盖项必须单列。
- **额外要求（hermes 特有）**：工作区外的持久副作用（例如你自己的 skill/记忆文件）
  不属于本轮交付，若发生了请在 `output` 里披露一行；不要把它算进 `files_*`。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。
- 要再往下委派，必须显式写 `--parent-depth 1`，不得使用 shell 变量默认值。

## 交互纪律（W1/W2 实测总结，必须遵守）

- **不要触发任何额外交互**：不启动「学习练习 / tutorial / 引导流程」，不弹 Question/Ask
  等人类输入。轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes / 2 don't ask again / 3 No」时选 1，**不要选 2**；
  也不要为了少弹窗把命令合并成一条巨大脚本。
- **不要 export 会被 git 读取的环境变量**（`GIT_AUTHOR_*` / `GIT_COMMITTER_*` / `GIT_CONFIG_*`）。
  用 `git -c user.name=... -c user.email=...` 或 `env VAR=... git ...` 限定作用域。
  （W2 的 T5 踩过：误 export 了 `GIT_CONFIG_*`，自查后 unset。）
- **共享 checkout 的编译噪声**：本 wave 另有 3 个 agent 同时改别的子目录。
  若 `cargo` 报错的文件**不在你的白名单里**，那不是你的问题：等 30 秒重试；
  连续 3 次仍失败就记录进 `output` 并有界等待（必要时报 `blocked`），**不要改别人的文件**。
- **共享 `target/`**：并发 `cargo test` 会等文件锁（正常）。但**变异测试/临时构建必须用
  独立 `CARGO_TARGET_DIR` 或独立副本**，否则会污染共享缓存（W1 的 V2 踩过）。
- 对拍用的临时仓库全部放 `tempfile::tempdir()` 或 `/tmp/<你的名字>/`，**不要**污染本仓库。
