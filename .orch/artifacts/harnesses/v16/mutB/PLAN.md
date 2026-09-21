# mini-git 项目规划

> 目标：从零实现一个 git 子集，**与真实 git 二进制互操作**（同一个 `.git` 目录，两边都能读）。
> 这份文档是唯一的事实来源，随实现推进持续更新（勾选里程碑、记录决策）。

---

## 0. 定位与非目标

**做什么**

- 讲清楚 git 的数据模型：对象数据库、引用、索引、工作区三方状态。
- 讲清楚关键算法：tree 构建、Myers diff、merge-base、三方合并、packfile 与 delta。
- 用真实 `git` 作为 oracle 做差分测试，保证字节级兼容。

**不做什么（v1 明确排除）**

- 性能追平 git（不追求 mmap 索引、并行 delta、bitmap）。
- `rebase -i`、`submodule`、`worktree`、`rerere`、hook 执行、`git gc --aggressive` 级别策略。
- Windows 原生支持（先覆盖 Linux/macOS；Windows 仅保证不崩，不做测试门禁）。
- SHA-256 仓库、partial clone、promisor remote。

**命名**：CLI 可执行文件叫 `mg`（不叫 `git`，避免遮蔽真实 git 影响互操作测试）。README 里说明与 `git` 的对应关系。

---

## 1. 技术选型

| 项 | 选择 | 理由 |
|---|---|---|
| 语言 | **Rust**（推荐）| 二进制解析/字节切片友好，无 GC 抖动；用类型系统表达 object/ref 状态；`Result` 比异常更适合协议解析 |
| 备选 | Go（迭代快、并发做 fetch 协商舒服） / Python（最短学习曲线，但 pack 索引会慢到影响体验） | 仅影响实现节奏，**不影响下面的模块划分** |
| 压缩 | `miniz_oxide` 或 `flate2`（zlib） | loose object 与 pack 都是 zlib stream |
| 哈希 | `sha1`（v1 只做 SHA-1） | 预留 `Digest` trait 以便将来接 SHA-256 |
| CLI | `clap`（derive） | 子命令结构直接映射下表 |
| 错误 | `thiserror`（库）+ `anyhow`（CLI 边界） | 协议层错误需要类型化 |
| 测试 | `cargo test` + 自建差分 harness（shell out 真实 git） | 见 §5 |
| 格式化/静态检查 | `cargo fmt` + `cargo clippy -D warnings` | CI 门禁 |

依赖越少越好；上面 5 个之外，任何新依赖都要在本文档「决策记录」里写理由。

---

## 2. 数据模型（核心，先把这一节吃透再写代码）

### 2.1 Loose object

对象内容 = `"{type} {size}\0{payload}"`（payload 是裸字节），存储时整体 zlib 压缩，落到
`.git/objects/<oid[0:2]>/<oid[2:]>`，写入用「临时文件 + `rename`」保证原子性（对象不可变，已存在则直接跳过）。

对象 id = 上面的**未压缩**字节的 SHA-1 十六进制（40 字符）。四种 type：`blob` / `tree` / `commit` / `tag`。

### 2.2 四种对象

**blob**：裸文件内容，不含文件名、权限、路径。

**tree**：`{mode} {name}\0{20 字节二进制 oid}` 重复拼接。mode 是 ASCII 字符串：

| mode | 含义 |
|---|---|
| `100644` | 普通文件 |
| `100755` | 可执行文件 |
| `120000` | 符号链接（blob 内容是链接目标） |
| `40000` | 子树（**注意是 5 位，不是 `040000`**） |

排序规则：按 name 字节序，但**子树按其名字末尾多一个 `/` 参与比较**（`/`=0x2F 大于 `.`=0x2E，所以 `foo.txt` 排在 `foo/` 前）。这条规则搞错会导致 tree id 与 git 不一致。

**commit**：

```
tree <oid>
parent <oid>            # 0..n 行，首父在前
author Name <email> 1700000000 +0800
committer Name <email> 1700000000 +0800
<可选扩展头，如 gpgsig / encoding，多行用前导空格续行>

<message，保留原始换行，结尾通常带 \n>
```

**tag**：`object` / `type` / `tag` / `tagger` + 可选签名 + message。

### 2.3 引用

- `.git/HEAD`：`ref: refs/heads/main\n`（symbolic）或裸 oid（detached）。
- `.git/refs/heads/*`、`.git/refs/tags/*`、`.git/refs/remotes/*`：内容为 `<oid>\n`。
- `.git/packed-refs`：`<oid> <refname>` 行 + `^<oid>` 记录 peeled tag；**loose 优先于 packed**。
- 引用更新：写 `refs/..tmp` 再 `rename`；更新分支时用 `update-ref` 语义做 compare-and-swap（校验旧值，防并发覆盖）。
- 需要支持：`HEAD`、`ORIG_HEAD`、`MERGE_HEAD`、`FETCH_HEAD`、`refs/stash`（只读即可）。

### 2.4 索引（`.git/index`，DIRC v2）—— 本项目最容易翻车的二进制格式

```
"DIRC" | version:u32be(=2) | entry_count:u32be
entry[entry_count]
TREE 扩展:[ "TREE" | size:u32be | <tree oid> ]      # 缓存，可用可写
其他扩展: 未知签名一律原样保留（读→写不丢数据）
trailer: 前面所有字节的 SHA-1
```

单个 entry：

| 字段 | 宽度 | 说明 |
|---|---|---|
| ctime sec / nsec | 4+4 | 来自 lstat |
| mtime sec / nsec | 4+4 | 用于 racy-git 判断 |
| dev / ino | 4+4 | v2 为 32 位 |
| mode | 4 | `0100644` 等，含类型位 |
| uid / gid | 4+4 | |
| file size | 4 | 低 32 位 |
| oid | 20 | 二进制 |
| flags | 2 | bit15 assume-valid，bit14 extended，bit13-12 stage(0..3)，低 12 位 name 长度（截断到 0xFFF）|
| path | 变长 | UTF-8 字节，无 NUL |
| padding | 1..8 | 补 NUL，使 **entry 总长为 8 的倍数**（至少一个 NUL）|

**Racy git**：若 entry 的 mtime 与文件当前 mtime 相同且 size 相同，不能只信 stat，必须重新哈希内容。不做这一步 `status` 会在快速 checkout→修改时误报干净。

**stage（1/2/3）**：只有冲突时同一路径出现多条；`status` 需要按 stage 分组输出 `UU/AA/DU/...`。

### 2.5 三方状态机（`status` 的本质）

| 比较 | 结果标签 |
|---|---|
| HEAD tree vs index | `staged`：A/M/D，重命名暂不做（v1 见 §7）|
| index vs 工作区 | `unstaged`：M/D，symlink/mode 变化算 M |
| 工作区有、index 无 | `untracked`（受 `.gitignore` 过滤）|

---

## 3. CLI 表面设计

| 命令 | 语义要点 |
|---|---|
| `mg init [path]` | 建 `.git/{objects,refs/heads,refs/tags}`、`HEAD`→`refs/heads/main`、`config`；幂等 |
| `mg hash-object [-w] [-t blob] [--stdin] <file>` | 输出 oid，`-w` 落盘 |
| `mg cat-file (-t\|-s\|-p\|-e) <oid>` | `-p` 要美化：tree 按 `mode type oid\tname` 打印 |
| `mg add <pathspec>...` | 递归展开目录、应用 gitignore、写 blob、更新 index（含删除）|
| `mg rm [--cached] <path>` | 只改 index，或同时删工作区文件 |
| `mg status [--porcelain] [-s]` | 默认人类可读；`--porcelain=v1` 供测试断言 |
| `mg commit -m <msg> [-a] [--amend] [--allow-empty]` | 由 index 建 tree；写 commit；更新分支；刷新 index 的 stat 与 TREE 扩展 |
| `mg log [--oneline] [-n N] [--graph 可省] [rev]` | 首父优先的时间序（`--date-order`）遍历 |
| `mg diff [--staged] [<rev>] [-- <path>]` | Myers diff + unified hunk 输出，用于对照 `git diff` |
| `mg branch [-d\|-m] [name]`、`mg switch -c <name>` / `mg checkout <rev>` | 更新 HEAD、index、工作区；有未提交改动时拒绝并给出提示 |
| `mg merge <rev>` | fast-forward 优先；否则三方合并，冲突则写 `MERGE_HEAD` + index stage 1/2/3 + 冲突标记 |
| `mg tag [-a] [-m msg] <name> [rev]` | 轻量/附注两种 |
| `mg reset [--soft\|--mixed\|--hard] <rev>` | 默认 mixed：只回 index |
| `mg clone <url> [dir]`、`mg fetch`、`mg pull`、`mg push` | 传输层见 §6 |
| `mg fsck` | 自校：遍历所有可达对象、校验 SHA-1、index trailer、ref 指向存在 |

全局：`--version`、`-C <dir>`（切换仓库目录）、`-v` 详细日志。所有破坏性操作（`reset --hard`、`checkout` 覆盖）先做「无法安全丢弃的改动检测」。

---

## 4. 里程碑

每个里程碑都给出**验收标准**，且标准全部用真实 `git` 交叉验证——这是本项目质量的核心杠杆。

### M0 骨架（~0.5 天）
- 交付：Cargo 工程、`mg --version`、`repo::discover()`（从 cwd 上溯找 `.git`）、错误类型雏形、CI 跑 `fmt`+`clippy`+`test`。
- 验收：`mg -C /path --version` 可运行；CI 绿。

### M1 对象层（~1–2 天）
- 交付：zlib 编解码、SHA-1、loose object 读写、`init` / `hash-object` / `cat-file`。
- 验收：
  - `printf 'hello\n' | mg hash-object --stdin` == `git hash-object --stdin`；
  - `mg hash-object -w f` 后 `git cat-file -p <oid>` 正确，且 `git fsck` 无报错；
  - 反向：真实 `git init` 的仓库里 `mg cat-file -p HEAD` 能读出内容。

### M2 索引 + add/status（~3–5 天，最硬）
- 交付：`.git/index` 读写（保 unknown 扩展）、stat 缓存、gitignore 匹配、工作区扫描、`add` / `rm` / `status`。
- 验收：对同一随机改动序列，`mg status --porcelain` 与 `git status --porcelain` **逐字节相同**；`mg add` 后 `git ls-files --stage` 输出与预期一致；真实 `git add` 产生的 index，`mg status` 能正确解读（含冲突 stage）。

### M3 commit / log / diff（~2–3 天）
- 交付：index→tree 递归构建、commit 编码、ref 更新、提交图遍历、Myers diff + unified 输出。
- 验收：`mg commit` 后 `git log --format=raw` 与 `git fsck` 均通过，且 `git log -1 --format=%T` 等于 `mg` 自算 tree；`mg log --oneline` 与 `git log --oneline` 一致；`mg diff` 与 `git diff` 对同一对 blobs 输出一致（空格/`\ No newline` 标记也要对）。

### M4 分支 / checkout / tag / reset（~2–3 天）
- 交付：symbolic ref 与 detached HEAD、`branch` / `switch` / `checkout` / `tag` / `reset`、工作区物化（含 mode、symlink）、安全覆盖检查。
- 验收：branch/checkout 交叉操作后 `git status` 干净；`git symbolic-ref HEAD` 正确；`reset --hard` 后工作区与 `git checkout` 结果 diff 为空。

### M5 merge（~4–6 天）
- 交付：merge-base（按提交时间的双端 BFS）、fast-forward、三方合并（diff3 风格）、冲突写入、`MERGE_HEAD`/`ORIG_HEAD`、`mg commit` 完成合并。
- 验收：与 `git merge` 在 fixture 仓库（含冲突场景）产出相同的 tree（无冲突时）与相同的冲突文件内容（有冲突时）；冲突后 `git status --porcelain` 与 `mg status` 一致；`git commit` 能接手 `mg` 留下的 MERGE_HEAD 完成合并（证明状态文件兼容）。

### M6 传输：packfile + clone/fetch/push（~5–8 天）
- 交付：pkt-line 编解码、ref 广告与协商（v0 + side-band-64k 够用，v2 可选）、pack 文件解析（对象头 varint、`OFS_DELTA`/`REF_DELTA` delta 还原）、`index-pack` 生成 `.idx` v2、`file://` 与 smart HTTP(S) 传输、`clone` / `fetch` / `pull` / `push`（含 non-fast-forward 拒绝）。
- 验收：`mg clone file:///tmp/repo` 后 `git fsck` 通过、`git log` 与源仓一致；`git push` 到 bare 仓后真实 `git clone` 结果完全一致（反向也成立）；对真实 HTTP remote（如本地起的 `git http-backend`）能 fetch 到新提交。
- 说明：先做 `file://`（本地进程内直读），再做 HTTP；delta 还原是硬骨头，安排独立单测 + 用真实 pack 做差分。

### M7 gc / 优化（~2–4 天，可裁剪）
- 交付：loose→pack 打包、`.idx` 生成、`packed-refs` 写入、`mg gc`、`mg fsck` 完整化；大仓库下的 index/status 提速（缓存 stat、减少重哈希）。
- 验收：`mg gc` 后 `git log --all` 与 `git fsck` 正常；对象数下降；`status` 在 1 万文件仓库下 < 1s。

**v1 = M0–M6 全部通过 + M7 中 gc/fsck 部分**。粗估 3–4 周业余时间，M2/M5/M6 三个里程碑各占约 1/4 工作量。

---

## 5. 测试策略

1. **差分（golden）测试是主力**：`tests/interop/` 里每个用例都跑「准备仓库 → 真实 git 与 mg 各执行一遍 → 比较 git fsck / porcelain 输出 / tree oid / 文件内容」。要让真实 git 一定可用；CI 里显式断言版本。
2. **属性测试**（`proptest`）：随机内容 → `hash-object` 与 git 一致；随机文件树 → `write-tree` 与 git 一致；index 读→写→读 幂等。
3. **二进制格式单测**：index DIRC 手写字节 fixture（含 padding、长路径 >0xFFF、stage 冲突）、tree 排序边界（`foo.txt` vs `foo/`）、pkt-line 边界（`0000`/`0001`/最大长度）、delta 正反例。
4. **Fixture 仓库**：脚本生成一个含分支、合并、冲突、tag、subdir、symlink、可执行位、CRLF、非 ASCII 文件名的「脏」仓库，M3–M6 都拿它跑差分。
5. **模糊测试**（可选）：`cargo-fuzz` 输入到 pack/index 解析器，保证不 panic（解析错误必须是 `Err`）。

---

## 6. 目录结构（Rust 版）

```
mini-git/
├── PLAN.md                  # 本文档
├── README.md                # 用法 + 「与 git 命令对照表」
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI 入口，只有参数解析与错误打印
│   ├── cli/                 # 每个子命令一个文件
│   ├── repo.rs              # discover / 配置 / 仓库上下文
│   ├── odb/                 # 对象数据库：loose.rs, pack/ (read, index, delta)
│   ├── object/              # blob/tree/commit/tag 编解码 + oid 类型
│   ├── index/               # DIRC 读写、stage、TREE 扩展
│   ├── refs/                # HEAD、loose/packed refs、CAS 更新
│   ├── worktree/            # 扫描、stat 缓存、ignore、检出/写回
│   ├── diff/                # myers.rs、unified.rs
│   ├── merge/               # merge_base.rs、three_way.rs
│   └── transport/           # pktline.rs、http.rs、local.rs、negotiate.rs
└── tests/
    ├── interop/             # 与真实 git 的差分用例
    ├── fixtures/            # 生成脏仓库的脚本与期望输出
    └── common/              # 起临时仓库、跑 git 的辅助工具
```

分层规则：`odb` 只认 oid 和字节，不认路径；`worktree`/`index` 才可以碰文件系统路径；`cli` 不直接读文件，只调用下层。

**controller 独占文件**：`Cargo.toml`、`src/main.rs`、`src/lib.rs`、各 `src/*/mod.rs` 的公共声明区、
`tests/interop/**`。该模块划分同时作为多 agent 并行开发的写作用域边界，
详见 `ORCHESTRATION.md`（含依赖 DAG、Wave 计划、异构分配、评测指标）。

---

## 7. 风险与难点（提前埋好对策）

| 风险 | 对策 |
|---|---|
| index 二进制格式细节（padding、长路径、扩展、trailer） | M2 一上来就用真实 git 产生的 index 做「读→写→`git ls-files` 比对」的闭环，不要靠自己臆造 fixture |
| racy-git 导致 status 误判 | 实现 stat 相等时重哈希；用 `touch` 同秒修改的用例专门测 |
| tree 排序与 mode 字符串 | 直接在 M1/M3 用 `git write-tree` 差分；单元测 `foo.txt` vs `foo/` |
| delta 解压/应用（OFS 负偏移 varint） | 独立模块 + 真实 pack 差分；先只读不写 delta，push 侧可先发完整对象 |
| merge-base 与三方合并语义 | 先只支持无冲突的 fast-forward + 简单三方；冲突处理对齐 `git status --porcelain` 的 stage 组合 |
| CRLF / autocrlf / 权限位 / 非 ASCII 文件名 | v1 明确「不做 autocrlf 转换」，在 README 声明；测试仓库覆盖 symlink、exec bit、中文文件名 |
| 忽略规则语义复杂 | v1 支持 `.gitignore` 基础模式（`*`、`?`、`[]`、`!`、`/` 锚定、目录尾 `/`），不支持 `.git/info/exclude` 与 global excludes（写进文档） |
| 重命名检测 | v1 不做（`status` 显示 D+A，与 `git status` 默认不同——差分测试需加 `-c status.renames=false` 或对比时归一化） |
| 并发/中断写坏仓库 | 所有写用 tmp+rename；commit 时先写对象再动 ref |

---

## 8. 决策记录（随实现追加）

| 日期 | 决策 | 理由 |
|---|---|---|
| 2026-09-19 | 语言选型待定（Rust 为默认推荐） | 见 §1 |
| 2026-09-19 | v1 排除 rebase/submodule/worktree、autocrlf、重命名检测 | 控制范围，先保证核心互操作 |

---

## 9. 待你确认

1. **语言**：默认按 Rust 落地（模块划分与语言无关）；是否改用 Go / Python？
2. **v1 边界**：远端（M6 clone/fetch/push）要进 v1 吗？还是先做到 M5（本地全功能）为止？
3. **学习导向 vs 成品导向**：是否希望每个里程碑附带一篇「原理笔记」（README 或 `docs/`），还是只要代码 + 测试？
4. **是否要「先写测试再实现」**：M1 起就把差分 harness 搭起来（推荐，成本低收益高）？
