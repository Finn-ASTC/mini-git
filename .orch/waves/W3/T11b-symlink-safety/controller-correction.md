# Controller 更正（v2，2026-09-19 21:35 定稿）：真值取决于夹具变量，不是「谁测错了」

> v1 版本曾判定「V11 的 FAIL 2 是测量错误」——**v1 自己也是错的**，见 §3。本文件保留全过程，
> 因为它记录的是本 wave 最有价值的一条方法论缺陷。

## 1. 争议

V11（codex 验证者）报了两个 FAIL：
- **FAIL 1**：删除路径跟随 symlink 祖先，删掉工作区外的数据（真缺陷）。
- **FAIL 2**：index 有 `a/b/c`、工作区 `a` 是指向工作区外的 symlink、目标 rev 修改 `a/b/c` 时，
  mg exit 1 拒绝，而**真实 git exit 0**（把 symlink 换成真目录并写入）。

## 2. Controller 独立复跑（真实 git 2.55.0，`LC_ALL=C`）

| 场景 | 顺 `a/b/c` 能否 stat 到目标 | 命令 | git exit | 工作区外 | `a` 还是 symlink |
|---|---|---|---|---|---|
| 空 outside 目录 | **不能**（ENOENT） | `git switch other` | **0** | 零变动 | **否**（换成真目录，写入工作区内） |
| 空 outside 目录 | 不能 | `git switch -f other` | 0 | 零变动 | 否 |
| outside/b/c v1 存在 | **能**（stat 成功） | `git switch other` | **1** 拒绝 | 零变动 | 是 |
| outside/b/c v1 存在 | 能 | `git switch -f other` | 0 | 零变动 | 否 |

复跑现场与命令：`/tmp/g6`（真实 git）、`/tmp/g7`（同一夹具跑 `mg`）；脚本见本文末尾。

**结论**：V11 的 FAIL 2 与 controller 的第一版「撤销」**都是实测正确的**，
差别只在夹具里工作区外那个文件**存在与否**。真实 git 的规则是：

- 顺 symlink 祖先 **能 stat 到**目标 → 视为「本地改动会被覆盖」→ 非 force **拒绝**（与内容是否相同无关）；
- 顺 symlink 祖先 **stat 不到**目标 → 磁盘上没有可覆盖之物 → 非 force 也**放行**，把 symlink 换成真目录后写入，
  写出的内容永远落在工作区内。

## 3. 两次「更正」的教训（本 wave 最重要的方法论产出）

1. V11 的 FAIL 2 报告**只给了结论，没有给夹具的完整状态**（工作区外那个目录是空的还是有文件）。
2. controller 复跑时**自己换了夹具**（outside/b/c 存在）并得到 exit 1，于是把「V11 测错了」当成事实，
   写进 v1 更正与 V11b 验证书。**这是同一个错误的第二次发生**，只是这次发生在 controller 身上。
3. 作者（T11b）没有站队，而是把夹具变量做成 42 格矩阵（`祖先类型 × 目标动作 × force`），
   于是「谁对谁错」变成了「在哪一格上对」——**这才是真值该有的形态**。
4. 结果：改码后 mg 在 42/42 格与 git 一致（controller 抽查 5 格：4 格写路径 + FAIL 1 删除路径，全部逐字节一致）。

**固化到模板的条款**（已写入 W4 验证任务书与 `SKILL-FINDINGS.md` P18）：
凡是「真实 git 会 X」的断言，必须写成**带夹具的状态矩阵 + 原始命令输出**，
不能写成一句结论；复跑者必须复用**同一个夹具**，或明确说明自己换了哪个变量。

## 4. 本轮真正要做的（最终版）

1. **FAIL 1 必须修**：删除路径遇 symlink 祖先时跳过，工作区外零变动（git 实测：非 force exit 1、
   force exit 0 且 symlink 保留）。✅ 已修（`ancestor_barrier()` 逐段 `symlink_metadata`，绝不跟随）。
2. **FAIL 2 成立且要修**，但**只修「stat 不到」那一格**：mg 必须与 git 一样，仅在
   「目标已在 index 且顺 symlink 祖先 stat 不到」时放行并换成真目录；
   「stat 得到」时照旧拒绝；未跟踪的新路径必须继续拒绝（git 报 `untracked working tree files would be overwritten`）。
3. **不得回退**：`write_blob_to()` / `ensure_parents()` 的库级「拒绝 symlink 祖先」守卫保持不变
   （验证者用例点名的直接调用仍须返回 `Err`）；换成真目录只允许发生在物化写路径的清场步骤。

## 5. 可复跑脚本

```bash
# 真实 git 侧（把 MG 换成 mg 的绝对路径即为 mg 侧）
for scenario in empty present; do for mode in switch "switch -f"; do
  d=/tmp/gx/$scenario-${mode// /_}; rm -rf $d; mkdir -p $d/outside; cd $d
  git init -q -b main r && cd r
  mkdir -p a/b && echo v1 > a/b/c && git add -A && git -c user.name=t -c user.email=t@e commit -qm one
  git -c user.name=t -c user.email=t@e checkout -q -b other && echo v2 > a/b/c && git -c user.name=t -c user.email=t@e commit -qam two
  git -c user.name=t -c user.email=t@e checkout -q main
  [ $scenario = present ] && { mkdir -p $d/outside/b && echo v1 > $d/outside/b/c; }
  rm -rf a && ln -s $d/outside a
  git -c user.name=t -c user.email=t@e $mode other; echo "$scenario $mode exit=$? symlink=$([ -L a ] && echo yes || echo no)"
done; done
```
