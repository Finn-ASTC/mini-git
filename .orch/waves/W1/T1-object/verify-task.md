# V1 —— 独立验证 T1（object 层）

你是本轮的**验证者**（kind: codex），不是实现者。作者是另一个 agent（omp）。
你的任务：**不接受作者的任何自述**，用真实 git 作为唯一真值，独立判定 T1 是否成立。

## 0. 被验证的对象

- 任务包：`.orch/waves/W1/T1-object/task.md`
- 作者的结果：`.orch/rounds/W1/agent-orchestrator-1vhlii69/result.json`
- 作者被允许改的文件（本轮**你不得修改**）：`src/object/tree.rs`、`src/object/commit.rs`、`src/object/tag.rs`
- 开工前基线：`.orch/waves/W1/T1-object/baseline.txt`（可用来确认没有越界改动）

## 1. 你的写作用域（白名单）

- `tests/verify/main.rs`（新建，若不存在则创建；这是**你自己的**测试 target）
- `.orch/waves/W1/T1-object/verify-scratch/**`（临时仓库，随便用）
- 你的 result 路径（由 controller 的 prompt.txt 给出）

**禁止修改**任何其它文件，尤其是 `src/**`、`Cargo.toml`、`Cargo.lock`、`tests/interop/**`、
`.orch/rounds/**`（那是别人轮次的不可变证据）。若你认为必须改 `src/**`，不要改：报 `blocked`。

## 2. 必须独立执行的检查（逐项给出原始输出）

### (A) 复跑作者的测试

```bash
cd /home/user/Projects/mini-git
cargo test --offline
cargo clippy --offline --all-targets      # 必须 0 warning
scripts/check-freeze.sh                   # 必须打印 freeze-v0 intact（drift 0）
```

### (B) 反假绿：逐个核对作者硬编码的 golden 常量是否真的来自 git

作者被要求把真实 git 的输出硬编码进单元测试。你要**自己重新生成**这些真值并比对：

```bash
SCRATCH=/home/user/Projects/mini-git/.orch/waves/W1/T1-object/verify-scratch
mkdir -p "$SCRATCH"
cd "$SCRATCH" && rm -rf tree-case && mkdir tree-case && cd tree-case && git init -q .
printf 'a\n' > a.txt; mkdir foo; printf 'b\n' > foo/b.txt; printf 'c\n' > foo.txt
printf '#!/bin/sh\n' > run.sh; chmod +x run.sh; ln -s a.txt link.txt
git add -A
git write-tree
git cat-file tree "$(git write-tree)" | xxd -p | tr -d '\n'; echo

# commit case
cd "$SCRATCH" && rm -rf commit-case && mkdir commit-case && cd commit-case && git init -q .
printf 'x\n' > x.txt; git add -A
GIT_AUTHOR_DATE='1700000000 +0800' GIT_COMMITTER_DATE='1700000000 +0800' \
  git -c user.name='A U Thor' -c user.email='a@example.com' commit -q -m $'subject\n\nbody line\n'
git cat-file commit HEAD | xxd -p | tr -d '\n'; echo

# tag case
git tag -a v1 -m 'tag message' HEAD
git cat-file tag v1 | xxd -p | tr -d '\n'; echo
```

把上面的结果与 `src/object/{tree,commit,tag}.rs` 里测试断言的值逐条比对，并在 output 里
列出「哪条常量与真值一致 / 哪条不一致」。**常量被改动过、或断言被放宽成「包含」之类，
都算验证失败。**

### (C) 断言是不是真的在跑被测代码

读作者的测试体，确认它们真的调用了 `encode_payload` / `decode_payload` / `sort_entries`
（而不是直接断言常量、或用自己构造的期望值自证）。在 output 里说明每类断言的证据。

### (D) 新增一个作者没测过的独立用例（必须做）

在 `tests/verify/main.rs` 里写一个新的测试 target，用 `minigit` 库（`use minigit::...`）
**在运行时**与真实 git 对拍，不允许硬编码真值：

至少覆盖：

1. **排序陷阱**：同一目录下同时存在 `foo.txt`（文件）与 `foo/`（子树），
   再加一个 `foo-bar`、一个深层 `foo/a/b`，用真实 `git write-tree` 得到的 oid
   与 `Oid::hash_object("tree", Tree::encode_payload())` 比较（要相等）。
2. **字节往返**：把 `git cat-file tree <oid>` 的原始字节交给 `Tree::decode_payload`，
   再 `encode_payload`，必须逐字节相同。
3. **commit 往返 + 多父提交**：造一个 merge 提交（两个 parent），
   用 `git cat-file commit HEAD` 的原始字节做往返，并断言 `parents.len() == 2`。
4. **tag 往返**：annotated tag 的原始字节往返一致。

实现方式建议：测试里用 `std::process::Command` 调 `git`（`git init` / `git add` / `git write-tree` /
`git cat-file`），仓库建在 `std::env::temp_dir()` 下。**若沙箱拒绝写 `/tmp`，不要把权限放宽**：
如实报 `blocked` 并说明。

## 3. 判定与上报

- 全部检查通过 → `status: success`，`output` 里给出：复跑的命令与结果、
(B) 的逐条常量比对表、(D) 新增测试的名称与它实际跑出的输出。
- **任何一项不通过 → `status: error`**，`error` 写清具体是哪个用例、期望值、实际值、
以及最小复现命令。不要为了「让测试过」而放宽断言或改别人的代码。
- `files_created` 里应包含 `tests/verify/main.rs`（若你创建了它）；
其余数组按实际改动填，只列你本轮真正改动的文件。
- 不确定但影响结论的事情（例如某个 case 你无法判定），用 `blocked` 上报并给出你的中间结论。
