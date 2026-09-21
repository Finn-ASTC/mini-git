# linestat 1.0.0 —— 交付规格

## 交付物

一个**无第三方依赖**的 Rust 二进制 crate，名字 `linestat`，位于本目录（`cargo init` 已可省略，
你负责建好 `Cargo.toml` / `src/main.rs` / 测试）。

- `cargo build --offline` → `target/debug/linestat`
- `cargo build --release --offline` → `target/release/linestat`（这是**实际交付物**）

## CLI 契约

```
linestat [--version] [FILE...]
```

### `--version`

stdout 输出恰好 `linestat 1.0.0` + 换行，退出码 0，不读任何文件。

### 正常路径

对每个 `FILE` **按参数顺序**输出一行：

```
<lines> <words> <bytes> <path>
```

四个字段之间用**单个空格**分隔，`<path>` 原样回显（不做规范化）。
当且仅当给出的 `FILE` **多于一个**时，最后再输出一行总计：

```
<lines> <words> <bytes> total
```

### 三个计数的定义（**必须逐字实现，不要用 `wc` 的语义**）

- `bytes`：文件字节数。
- `lines`：`\n`（0x0A）出现的次数；若文件**非空**且**不以 `\n` 结尾**，再 **+1**。空文件为 `0`。
- `words`：把字节流切成「非空白字节的极大连续段」，段数即 `words`。
  空白字节 = 空格(0x20)、`\t`(0x09)、`\n`(0x0A)、`\r`(0x0D)、`\v`(0x0B)、`\f`(0x0C)。
  **按字节判定，不做 Unicode 分词**；非 UTF-8 字节序列照常参与计数。

### 错误路径

- 某个 `FILE` 打不开（不存在 / 不是文件 / 无权限）：向 **stderr** 写一行
  `linestat: <path>: <错误描述>`，**继续处理后面的文件**，全部处理完后退出码为 **1**。
- 未知选项（例如 `--help`、`-x`）：向 stderr 写一行
  `linestat: usage: linestat [--version] [FILE...]`，退出码 **2**。
- 没给任何 `FILE` 且没有 `--version`：同样按 usage 错误处理（退出码 2）。
- 全部成功：退出码 **0**。

## 交付配置（本项目的验收矩阵）

| 配置 | 门禁 | owner |
|---|---|---|
| dev | `cargo test --offline` 通过 | 作者跑，验证者复跑 |
| release | `cargo build --release --offline` 成功 **且** `cargo test --release --offline` 通过 | 作者跑，验证者复跑 |
| release 制品 | 用**构建出来的** `target/release/linestat` 跑行为检查 | 作者跑，验证者复跑 |

- 「构建成功」**不算** release 制品的行为检查。
- 未覆盖的配置要**显式说明**（例如「不适用，原因 …」），不能省略。
- 项目要求 `cargo fmt --all -- --check` 与 `cargo clippy --offline --all-targets -- -D warnings` 干净。

## 报告要求

result.json 的 `output` 里必须包含：每个配置的实际命令、实际退出码、
release 制品的路径与 SHA-256、以及该配置下未覆盖/跳过的项与原因。
不能只报一个测试总数。
