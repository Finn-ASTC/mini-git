# T1 —— object 层：Tree 排序/编解码、Commit、Tag、Signature

你是本轮的实现者（kind: omp）。仓库 `/home/user/Projects/mini-git` 是一个
「与真实 git 互操作」的教学实现（CLI 名 `mg`）。本轮只做 `src/object/` 里的三块内容。

## 1. 目标

实现以下**已经冻结签名**的函数体（签名不许改）：

| 文件 | 需要实现的符号 |
|---|---|
| `src/object/tree.rs` | `Tree::sort_entries`、`Tree::encode_payload`、`Tree::decode_payload`、`Tree::lookup`（正确返回）|
| `src/object/commit.rs` | `Signature::parse`、`Signature::format`、`Commit::encode_payload`、`Commit::decode_payload` |
| `src/object/tag.rs` | `Tag::encode_payload`、`Tag::decode_payload` |

正确性标准是**与真实 git 的字节表示完全一致**，不是「看起来合理」。

## 2. 写作用域（白名单，只允许改这三个文件）

- `src/object/tree.rs`
- `src/object/commit.rs`
- `src/object/tag.rs`

可以在这三个文件内部加 `#[cfg(test)] mod tests`。**其余文件一律不许改**，特别是：
`src/object/mod.rs`、`src/object/blob.rs`、`Cargo.toml`、`Cargo.lock`、`src/lib.rs`、
`src/main.rs`、`src/cli/**`、`tests/**`、`.orch/**`、`ORCHESTRATION.md`、`PLAN.md`。

如果你认为必须改白名单外的文件（例如需要新依赖），**不要改**：用 `blocked` 上报。

## 3. 关键格式（照做，不要自己发明）

**tree payload**：`"<mode> <name>\0"` + 20 字节二进制 oid，重复拼接。
mode 字符串：`100644` / `100755` / `120000` / `40000`（子树是 **5 位**，不是 `040000`）。
排序：按 name 字节序，但**子树按其名字末尾多一个 `/` 参与比较**
（`/`=0x2F > `.`=0x2E，所以 `foo.txt` 必须排在 `foo` 子树之前）。`Tree::encode_payload`
必须先按该规则排序再拼接（`sort_entries` 是它的依赖）。

**commit payload**：

```
tree <oid>\n
parent <oid>\n          (0..n 行，首父在前)
author <name> <<email>> <unix秒> <±HHMM>\n
committer <name> <<email>> <unix秒> <±HHMM>\n
<可选扩展头：值多行时后续行以一个空格开头>\n
\n
<message 原样>
```

`Signature` 解析要能处理 `Name <email> 1700000000 +0800`，并且**编解码往返逐字节一致**。
`extra_headers` 必须按读入顺序保留（不能丢 `gpgsig`、`encoding` 这类头）。注意
`decode_payload` 之后 `encode_payload` 必须还原原始字节——**包括 message 末尾是否有换行**。

**tag payload**：`object <oid>` / `type <kind>` / `tag <name>` / `tagger <sig>` / 空行 / message，
同样要求往返逐字节一致（`tagger` 可能缺失，此时不能凭空生成）。

## 4. 验收（必须自己跑，并在 output 里给出真实输出）

在仓库根目录执行：

```bash
cargo test --offline
cargo clippy --offline --all-targets
```

要求：test 全绿、clippy **0 warning**。

另外必须用**真实 git 的输出**做差分，不许只写「自己跟自己比」的测试。用下面的命令在临时目录
生成真值，然后把真值**硬编码**进你写的单元测试里（golden test）：

```bash
# (a) tree：含子目录、可执行位、symlink，以及 foo.txt 与 foo/ 的排序陷阱
TMP=$(mktemp -d); cd "$TMP"; git init -q .
printf 'a\n' > a.txt; mkdir foo; printf 'b\n' > foo/b.txt; printf 'c\n' > foo.txt
printf '#!/bin/sh\n' > run.sh; chmod +x run.sh; ln -s a.txt link.txt
git add -A
git write-tree                       # => 你的 Tree::encode_payload 必须算出同一个 oid
git cat-file tree "$(git write-tree)" | xxd -p | tr -d '\n'   # => 与 encode_payload 逐字节相同

# (b) commit：真实提交的原始字节
TMP2=$(mktemp -d); cd "$TMP2"; git init -q .
printf 'x\n' > x.txt; git add -A
GIT_AUTHOR_DATE='1700000000 +0800' GIT_COMMITTER_DATE='1700000000 +0800' \
  git -c user.name='A U Thor' -c user.email='a@example.com' commit -q -m $'subject\n\nbody line\n'
git cat-file commit HEAD | xxd -p | tr -d '\n'   # => decode 再 encode 必须逐字节还原

# (c) annotated tag
git tag -a v1 -m 'tag message' HEAD
git cat-file tag v1 | xxd -p | tr -d '\n'        # => 同样逐字节往返
```

你要写的测试至少覆盖：

1. 上面 (a) 的 tree oid 与原始字节（含 `foo.txt` vs `foo` 的排序陷阱、可执行位、symlink）。
2. `Tree::decode_payload(原始字节).encode_payload() == 原始字节`。
3. 上面 (b) 的 commit 往返逐字节一致；且 `signature`、`parents`、`message` 解析正确。
4. 上面 (c) 的 tag 往返逐字节一致。
5. 一个「乱序输入必须被排成 git 顺序」的用例（直接构造乱序 entries 数组）。

## 5. 非目标

- 不要实现 `src/` 下的其他 `todo(...)`（那些属于其他任务）。
- 不要新增依赖、不要改公共签名、不要改 `Cargo.toml`。
- 不要为了让测试通过而放宽断言（例如把「逐字节相同」改成「包含」）。
- 不要修改 `tests/interop/**`。

## 6. 上报

- **result 路径与完整上报契约由 controller 生成的 prompt.txt 指定**，不要自己猜路径。
  prompt 里给了 `result_path` 与 `job_id`/`round_id`，按 agent-controlled 协议原子发布 JSON。
- `output` 里请给出：实际跑过的命令 + 真实结论（例如 `cargo test` 的通过数、
  `git write-tree` 得到的 oid 与你的实现算出的 oid 是否相同）。不要贴完整逐字稿。
- 本轮只改上面三个文件，`files_modified` 也只应包含它们。
