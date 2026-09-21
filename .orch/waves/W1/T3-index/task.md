# T3 —— index 层：`.git/index` DIRC v2 无损读写

你是本轮的**实现者**（kind: opencode）。仓库 `/home/user/Projects/mini-git` 是一个
「与真实 git 互操作」的教学实现（CLI 名 `mg`）。本轮只做 `src/index/dirc.rs` 的两个函数。

## 1. 目标

| 文件 | 需要实现的符号 |
|---|---|
| `src/index/dirc.rs` | `read_index(repo) -> Result<Index>`、`write_index(repo, &Index) -> Result<()>` |

（`src/index/mod.rs` 里的 `Index`/`IndexEntry`/`StatData`/`Extension` 类型与辅助方法
**已经实现好且冻结**，直接用，不要改。）

正确性标准 = **无损往返**：读真实 git 写的 index，再写回去，文件字节与原来**完全相同**，
并且 `git ls-files --stage` 输出不变、`git status` 仍然认为工作区干净。

## 2. 写作用域（白名单，只允许改这一个文件）

- `src/index/dirc.rs`

可以在这个文件内部加 `#[cfg(test)] mod tests`（用 `tempfile` + 真实 `git` 二进制做对拍；
`tempfile` 已在 dev-dependencies 里，**不要动 Cargo.toml**）。**其余文件一律不许改**，特别是：
`src/index/mod.rs`（CONTROLLER-OWNED）、`src/object/**`（另一个 agent 本轮在改）、
`src/worktree/**`（第三个 agent 本轮在改）、`src/repo.rs`、`src/error.rs`、`src/oid.rs`、
`src/lib.rs`、`src/main.rs`、`src/cli/**`、`tests/**`、`.orch/**`、`Cargo.toml`、`Cargo.lock`。

如果你认为必须改白名单外的文件（例如需要新依赖、或 index 类型缺字段），**不要改**：用 `blocked` 上报。

## 3. 格式（文件头注释里已有格式表，这里是必须踩准的细节）

```
"DIRC" | version:u32be | entry_count:u32be | entry* | extension* | sha1(everything before)
```

**entry**（固定 62 字节 + path + padding，全部大端）：

| 字段 | 宽度 | 备注 |
|---|---|---|
| ctime sec / nsec | 4+4 | 直接搬运，不需要重算 |
| mtime sec / nsec | 4+4 | 同上 |
| dev / ino | 4+4 | 注意 `MetadataExt` 给的是 u64，落盘只取低 32 位（`as u32`）|
| mode | 4 | 含类型位：`0o100644`/`0o100755`/`0o120000`；用 `FileMode::from_u32`/`to_u32` |
| uid / gid | 4+4 | |
| size | 4 | 低 32 位 |
| oid | 20 | 二进制，不是 hex |
| flags | 2 | bit15 assume-valid，bit14 extended，bit13-12 stage，低 12 位 = name 长度 |
| path | 变长，**无 NUL 终止** | |
| padding | 1–8 个 `\0` | 使**(62 + path 长度 + padding) 是 8 的倍数**；path 长度本身已是 8 倍数时也要补 8 个 |

- name 长度 ≥ 0xFFF 时，flags 低 12 位写 `0xFFF`（截断），path 用 NUL 终止；
  读的时候遇到 `0xFFF` 要按 NUL 找结尾。**这是必须处理的边界**。
- 读的时候把原始 2 字节 flags 存进 `IndexEntry::flags_raw`，把 bit15/bit14 分别
  填进 `assume_valid` / `extended`，stage 填进 `stage`。
- 写的时候**不要无脑回写 `flags_raw`**：至少要按实际 path 长度重算低 12 位
  （否则长度 >0xFFF 或发生变化的条目会写坏），其余位从 `flags_raw` 继承。
  往返测试会替你验证这一点。

**extension**：`signature:4 + length:u32be + data[length]`，按原顺序。

- `b"TREE"` 扩展 → 解析出 20 字节 oid 存进 `Index::tree_oid`；
  **写回时如果 `tree_oid` 是 `Some`，必须重新生成 TREE 扩展**（真实 git 依赖它做 status 加速，
  但它只是缓存，丢了不算损坏——不过往返字节相等要求你原样保住）。
- 其它签名（含 `REUC`、`link`、`UNTR`、以及未知的）→ 原样存进 `Index::extensions`，
  写回时按原顺序原样写出。**未知扩展绝不能丢**，丢一个字节 trailer 就对不上。
- 写回时扩展区顺序：TREE 在前（如果存在），其余按读入顺序。往返测试以真实 git 文件为准。

**trailer**：前面所有字节（含 header 与 padding）的 SHA-1，20 字节。项目已有 `sha1` 依赖，
`src/oid.rs` 的 `Oid::hash_object(kind, payload)` 可参考（但这里要的是裸 SHA-1，
**不是** `sha1("blob\0"+…)`，注意别用错）。校验失败 → `Error::corrupt("index", ...)`。

**版本**：只支持 2。`version != 2` → `Error::Unsupported("index version N is not supported (v1 supports 2)")`。
（`src/index/dirc.rs` 里的 `SUPPORTED_VERSION` 常量已经写好。）

**文件不存在**：`read_index` 返回 `Index::default()`（空 index，**不是错误**）。
`Index::default()` 的 `version` 是 0，注意 `write_index` 要写出的是 2。

**原子写**：写 `<git_dir>/index.lock`（或临时文件）再 `rename` 到 `.git/index`。
不要直接覆盖目标文件。

## 4. 验收（必须能独立复跑）

```bash
cd /home/user/Projects/mini-git
cargo test --offline index::          # 必须全绿
cargo clippy --offline --all-targets  # 必须 0 warning
scripts/check-freeze.sh               # 必须打印 drift 0
```

测试里必须至少有这几类（真值全部来自真实 `git` 二进制，**不许手写期望字节**）：

1. **字节级往返（最重要）**：`git init` → 造出多样化的索引 → `cp .git/index` 到备份 →
   `read` → `write` → 断言 `.git/index` 与备份**字节完全相等**（`assert_eq!(fs::read(...))`）。
   索引内容要覆盖：嵌套目录、可执行位（`chmod +x`）、symlink、路径长度 > 0xFFF 的深路径、
   非 ASCII（UTF-8，例如 `中文文件.txt`）。至少各来一个。
2. **语义往返**：`git ls-files --stage` 的输出在 mg 重写前后完全相同；
   `git status --porcelain` 仍是空。
3. **git 的二次确认**：让真实 git 动过重写后的索引仍然正常，例如
   `git update-index --refresh` / `git add` 一个新文件后 `git status` 正常、`git fsck` 无报错。
4. **反例（必须失败）**：把 index 里任意一个字节改掉（例如 trailer 最后一个字节）→ `read` 必须返回
   `Error::Corrupt`；把 `version` 字段改成 3 → 必须返回 `Error::Unsupported`；
   把文件截断到 4 字节 → 必须报错而不是 panic。
5. **空 index**：文件不存在时 `read` 不报错；`write` 出来的文件真实 git 能读
   （可以跑 `git ls-files --stage` 得到空输出验证）。

## 5. 非目标

- 不支持 index v3/v4、split index、sparse index、`gext` 语义解析。
- 不要动 `Cargo.toml`；需要新依赖请报 `blocked`。
- 不改任何 `pub` 签名，不新增模块级 `pub` 项。
- 不要去改 `src/worktree/` 或 `src/object/`（本轮有别的 agent 在改那里）。

## 6. 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**，不要自己猜、不要写到别处。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output` 必须包含：跑了哪些命令 + 真实结论（测试数量、往返是否字节相等、反例是否真的报错）。
  不要贴完整逐字稿。

## 7. 深度与上限

- 本轮的 depth：1
- `max_depth`：3
- 若需要再往下委派：必须把 `--parent-depth 1` 显式写死，不得使用 shell 变量默认值。

## 8. 交互纪律（控制器实测总结，必须遵守）

- **不要触发任何额外交互**：不启动「学习练习 / tutorial / 引导流程 / 交互式向导」，
  不弹 Question/Ask 对话框等待人类输入。轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes / 2 don't ask again / 3 No」时选 1，不要选 2。
- **共享 checkout 的编译噪声**：本 wave 另有 2 个 agent 同时改 `src/refs/`、`src/worktree/`。
  若 `cargo` 报错的文件**不在你的白名单里**，那不是你的问题：等 30 秒重试；
  连续 3 次仍失败则不要改别人的文件，在 `output` 里记录并有界等待（必要时报 `blocked`）。
- **cargo target 目录是共享的**：并发 `cargo test` 会等文件锁，属正常现象，不是错误。
