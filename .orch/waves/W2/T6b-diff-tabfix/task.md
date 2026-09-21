# T6b —— 修复 `mg diff` 在含空格路径上的 `---`/`+++` 行缺 TAB

你是本轮的**实现者**（kind: omp）。这是 T6（diff 引擎）的**返工轮**：独立验证者（codex）
在真实 git 对拍中抓到一个**逐字节差异**，需要你按 git 的真实规则修掉。

## 0. 缺陷（已被验证，直接照做即可）

验证者结论（`.orch/waves/W2/T6-diff/verify-scratch/known-fail-space-in-path.txt`）：

```
printf 'line1\nline2\n' > 'a b.txt'; git add -A
git -c user.name=V -c user.email=v@e commit -m base
printf 'line1\nCHANGED\n' > 'a b.txt'
git diff --no-renames > git.out ; target/debug/mg diff > mg.out ; cmp git.out mg.out
→ cmp: differ: char 75, line 3
git: --- a/a b.txt\t        mg: --- a/a b.txt
git: +++ b/a b.txt\t        mg: +++ b/a b.txt
```

实测规则（git 2.55.0）：**`--- `/`+++ ` 行的名字里只要含空格字节，git 就在行尾补一个 TAB**
（`a b.txt`、`a/dir x/f.txt`、`a/trail .txt`、以及 C-quoted 形式 `"a/a \"b c.txt"` 都补；
名字不含空格时不补；`diff --git` 行**从不**补；`git diff --no-index` 也补）。
其余部分（`index` 行、hunk 头、正文、`Binary files ...`）验证者已实测一致。

## 1. 目标

| 文件 | 要做的事 |
|---|---|
| `src/diff/unified.rs` | 让 `--- `/`+++ ` 行按上述规则补 TAB（这是 label 写出的地方） |
| `src/cli/diff.rs` | 只有当 label 的构造方式需要配合时才动；**不要**顺手改别的逻辑 |

签名不许改；`src/diff/mod.rs`（CONTROLLER-OWNED）不许动。

## 2. 写作用域（白名单，只有这两个文件）

- `src/diff/unified.rs`
- `src/cli/diff.rs`

**不许改**任何其它文件，特别是 `tests/verify_diff.rs`（那是验证者的文件，只读跑它）、
`tests/interop/**`、`src/diff/myers.rs`（除非缺陷确实在 myers —— 若你判断是，先在 `output` 说明理由）、
`Cargo.toml`、`Cargo.lock`、`.orch/**`（除了你自己的 result/scratch）。

## 3. 要求

1. **最小改动**：只修这个规则，不要重构、不要改格式、不要顺带修别的「看起来不对」的地方
   （那些写进 `output` 的「观察」即可）。
2. **在 `src/diff/unified.rs` 里加回归测试**（`#[cfg(test)] mod tests`，若已有就追加），
   按真实 git 对拍覆盖这一组名字（**真值都来自运行时 git 进程，不许硬编码期望字节**）：
   无空格、含空格、目录分量含空格、结尾空格、含 `"`、含 `\`、含 TAB、CJK、非 UTF-8 字节、
   `--no-index` 模式、`-U0`/`-U3`；并断言 `diff --git` 行**没有**被补 TAB。
3. **验证者留下的回归用例必须变绿**（不要改它，只跑它）：
   ```bash
   cargo test --offline --test verify_diff cli_space_in_path_tab_padding -- --exact --ignored
   ```
   该用例当前是 `#[ignore]`（验证者为了避免把共享 `cargo test --offline` 弄红），
   修好后这条命令必须 **0 failed**；`output` 里贴原始输出。
4. 门禁全绿：
   ```bash
   cargo test --offline
   cargo clippy --offline --all-targets   # 0 warning
   scripts/check-freeze.sh                # drift 0
   ```
   注意 `cargo test --offline` 现在会带上 `verify_diff`（16 passed + 1 ignored）；
   修完这次改动后，**ignored 的那条仍会是 ignored**（别去改别人的文件），这是预期的。
5. 额外自测（**必须做**）：用 `git diff --no-renames` 与 `target/debug/mg diff` 在一个含
   空格路径的真实仓库里 `cmp` 逐字节，覆盖「工作区 vs index」「`--staged`」「`HEAD`」三种组合。

## 4. 非目标

- 不做 rename 检测、不改 hunk 逻辑、不动 `diff --git` 行的引号规则（除非实测证明它也需要配合）。

## 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：跑了哪些命令 + 真实结论（哪些名字组合逐字节一致、回归用例输出、门禁数字）。
  已知限制/未覆盖项单列。**明确说明你是否改了 `src/cli/diff.rs`，以及为什么。**

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1/W2 实测总结）

- 不触发任何额外交互（不启动学习练习/引导流程）；omp 轮次结束后若弹「学习练习」对话框 → 选「不要」。
- 原生审批只按「单条」处理（选 1，不选 2）。
- 不 export `GIT_*` 环境变量；用 `git -c key=value` 或 `env VAR=... git ...`。
- 共享 checkout：本 wave 另有 V6/V5/V7 在跑。编译错误若出自你的白名单之外，
  注明「不可归因于本任务」，等 30 秒重试；不要改别人的文件。
- 变异测试/临时构建用独立 `CARGO_TARGET_DIR` 或独立副本。
