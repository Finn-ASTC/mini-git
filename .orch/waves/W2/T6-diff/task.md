# T6 —— diff：Myers + unified 输出 + `mg diff`

你是本轮的**实现者**（kind: omp）。仓库 `/home/user/Projects/mini-git` 是「与真实 git 互操作」的
实现（CLI 名 `mg`）。本轮做 diff 引擎与 `mg diff` 命令。

## 1. 目标（签名不许改）

| 文件 | 需要实现的符号 |
|---|---|
| `src/diff/myers.rs` | `myers(old, new) -> Result<Vec<Edit>>` |
| `src/diff/unified.rs` | `hunks(edits, old, new, context) -> Result<Vec<Hunk>>`、`unified(old, new, a_label, b_label, context) -> Result<String>` |
| `src/cli/diff.rs` | `run(staged, context, rev, paths) -> Result<()>` |

`src/diff/mod.rs` 的 `split_lines` / `line_count` / `diff_texts` **已实现**，直接用（它转发到你的
`unified()`）。文件头的注释里有完整格式表，**先读完再写**。

## 2. 写作用域（白名单，只允许改这三个文件）

- `src/diff/myers.rs`、`src/diff/unified.rs`、`src/cli/diff.rs`

可以在这三个文件内部加 `#[cfg(test)] mod tests`。**其余文件一律不许改**，特别是
`src/diff/mod.rs`（CONTROLLER-OWNED）、`src/cli/mod.rs`、`src/odb/**`（T5 正在改）、
`src/index/**`、`src/worktree/**`、`src/merge/**`、`Cargo.toml`、`Cargo.lock`、`tests/**`、`.orch/**`。
需要改白名单外文件 → 报 `blocked`。

## 3. 关键语义

### 3.1 `myers`
- 比较单位是**行**（`split_lines` 的定义：行尾 `\n` 属于该行，末行无 `\n` 也算一行）。
- 返回的 `Edit` 序列必须**完整覆盖**两侧行区间、按序、且 `Equal` 段的内容两侧一致。
- 允许实现简化版（比如只做 LCS 回溯，不做线性空间优化），但**必须与真实 git 的输出对齐**
  ——统一 diff 的文本才是最终判定标准，`Edit` 只是中间表示。
- 大输入要有保护：`O(N*M)` 的内存不能炸（比如 20000 行 × 20000 行的输入应能在秒级返回；
  若你选择加保护上限，必须在 `output` 里说明并保证真实 git 能处理的常规规模不触发）。

### 3.2 `hunks` / `unified`（格式细节，差分测试会逐字节比）
- 无改动 → **空字符串**（调用方据此不输出 header）。
- hunk 头：`@@ -<old_start>,<old_len> +<new_start>,<new_len> @@`；
  长度为 1 时**省略** `,1`；长度为 0 时起点写「前一行」（即 `-<start>,0`，start 是插入位置减一之后的值，
  与 git 一致——用真实 git 对拍确认，不要凭直觉写）。
- 相邻 hunk 间隔 ≤ 2×context 时**合并**成一个 hunk。
- 末尾无换行的那一行之后输出 `\ No newline at end of file`（`--- `/`+++ ` 行也要按 git 规则带）。
- 行前缀：context 空格、删除 `-`、添加 `+`。

### 3.3 `mg diff`
- 默认：工作区 vs index（不读 HEAD）。
- `--staged`：index vs HEAD。
- 给 rev：commit vs 工作区；两个 rev（`rev:Option<&str>`，本版本只有这一个参数，
  按骨架签名实现：`Some(rev)` 时与该 commit 比较）。
- 输出前必须打印 git 风格的头部：`diff --git a/<path> b/<path>`、
  `index <old7>..<new7> <mode>`（mode 形如 `100644`；新建文件是 `0000000..<new7>`）、
  `--- a/<path>` / `+++ b/<path>`；二进制文件输出 `Binary files a/x and b/x differ`。
- 路径过滤：`paths` 非空时只输出这些路径（本版本做**精确路径前缀匹配**即可）。
- 依赖：index 用 `crate::index::Index::read`（T3 已实现），odb 用 `crate::odb::Odb::read`
  （**T5 本轮并行实现中**：若它还是 `todo`，你的 `mg diff` 端到端测不了 —— 见 §4 的降级规则）。

## 4. 验收（必须能独立复跑）

```bash
cd /home/user/Projects/mini-git
cargo test --offline diff::
cargo test --offline
cargo clippy --offline --all-targets   # 0 warning
scripts/check-freeze.sh                # drift 0
```

1. **引擎级（必须做，且不依赖 T5）**：在 `src/diff/unified.rs` 的测试里，用**你自己构造**的
   old/new 字节序列生成 unified 文本，并与真实 git 对拍：
   ```bash
   git diff --no-index --unified=<context> -- <a_file> <b_file>
   ```
   注意 `--no-index` 的输出 header 是 `a/`、`b/` 之外的路径形式——**对齐规则**：
   `git diff --no-index` 会打印真实路径，所以你要么在测试里把 header 归一化后再比，
   要么用 `git diff` 在**临时仓库**里比（推荐后者，最真实）。
   至少覆盖：纯新增/纯删除/中间修改/多 hunk/相邻 hunk 合并/上下文为 1 与 3/
   末尾无换行/空文件 ↔ 非空/完全相同的两个文件/CJK 与含 `\r` 的行。
2. **端到端（尽力而为）**：临时仓库里用真实 git 造出「已提交 + 工作区改动」，
   `mg diff` 与 `git diff` 逐字节比较（`--staged` 也测一组）。
   **若 T5 的 odb 还是 `todo`**：不要越界去实现它，也不要用假数据；
   在 `output` 里记为「端到端未覆盖，原因：odb 未实现（T5）」，并把引擎级对拍结果给出。
3. 反例：二进制内容 → 必须走「Binary files ... differ」而不是打印乱码；
   巨大输入 → 不得 panic。

## 5. 非目标

- 不做 rename/copy 检测、不做 `--color`、`--stat`、`--word-diff`、不做 `diff.algorithm=histogram`。
- 不改 `Cargo.toml`；需要新依赖请报 `blocked`。

## 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**，不要自己猜、不要写到别处。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`（见 agent-controlled 协议）。
- `output` 必须包含：跑了哪些命令 + **真实结论**（数量、逐字节是否一致、反例是否真的失败）；
  不要贴完整逐字稿，也不要贴大段源码。已知限制/未覆盖项必须单列。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。
- 若要再往下委派，必须显式写 `--parent-depth 1`，不得使用 shell 变量默认值。

## 交互纪律（W1 实测总结，必须遵守）

- **不要触发任何额外交互**：不启动「学习练习 / tutorial / 引导流程」，不弹 Question/Ask
  等人类输入。轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes / 2 don't ask again / 3 No」时选 1，**不要选 2**；
  也不要为了少弹窗把命令合并成一条巨大脚本。
- **不要 export 会被 git 读取的环境变量**（`GIT_AUTHOR_*` / `GIT_COMMITTER_*` / `GIT_CONFIG_*`）。
  用 `git -c user.name=... -c user.email=...` 或 `env VAR=... git ...` 限定作用域。
  （W1 的 V4 踩过：泄漏导致别人的测试假红。）
- **共享 checkout 的编译噪声**：本 wave 另有 3 个 agent 同时改别的子目录。
  若 `cargo` 报错的文件**不在你的白名单里**，那不是你的问题：等 30 秒重试；连续 3 次仍失败
  就记录进 `output` 并有界等待（必要时报 `blocked`），**不要改别人的文件**。
- **共享 `target/`**：并发 `cargo test` 会等文件锁（正常）。但**变异测试/临时构建必须用
  独立 `CARGO_TARGET_DIR` 或独立副本**，否则会污染共享缓存（W1 的 V2 踩过）。
