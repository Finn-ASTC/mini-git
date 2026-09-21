# T15 —— `mg fsck` / `mg gc` + 全量 e2e 收口

你是本轮的**实现者**（kind: omp）。仓库 `/home/user/Projects/mini-git` 是「与真实 git 互操作」
的实现（CLI 名 `mg`）。本轮做**收口**：完整性校验（fsck）与仓库压缩（gc），并把整套 CLI 的
端到端一致性验证补上。

## 1. 目标（签名不许改）

| 文件 | 需要实现的符号 |
|---|---|
| `src/cli/fsck.rs` | `run(full: bool) -> Result<()>` |
| `src/cli/gc.rs` | `run() -> Result<()>` |

`src/cli/mod.rs`、`src/odb/mod.rs`、`src/odb/pack/mod.rs` 是 **CONTROLLER-OWNED**：不要改；
也不要新增 `src/odb/**` 的公共 API。**pack 的写出逻辑请写在 `src/cli/gc.rs` 内部的私有函数里**
（pack 头/对象记录/zlib/尾 sha1 + `.idx` v2：fanout/oid/crc32/offset/大 offset/尾 sha1），
读取侧复用 T9（已完成）的 `PackSet`/`PackFile`/`PackIndex`。

## 2. 写作用域（白名单）

- `src/cli/fsck.rs`
- `src/cli/gc.rs`
- `tests/interop/fsck_gc.rs`（**新建**；`tests/interop/main.rs` 是 CONTROLLER-OWNED，若需要注册
  test target 请**不要**改它 —— 直接用 `tests/` 下的新文件即可被 cargo 自动发现）

**不许改**其它文件：`src/odb/**`、`src/refs/**`、`src/index/**`、`src/worktree/**`、
`src/transport/**`、`src/merge/**`、`src/cli/mod.rs`、其它 `src/cli/*.rs`、`Cargo.toml`、
`Cargo.lock`、别人的 `tests/verify_*.rs`、`.orch/**`。

## 3. 关键语义

### 3.1 `mg fsck`
1. **refs**：所有 `refs/**` 与 `HEAD` 指向的对象必须存在（用 `RefStore::list` + `Odb::read`）。
2. **可达对象**：从全部 ref 出发遍历 commit → tree → blob/tag；
   - 每个对象的 `Oid::hash_object(kind,payload)` 必须等于它的 oid（用 `Odb::verify` 或自己做）；
   - tree 的每条 entry：mode 合法、oid 存在、名字不含 `/` 与 `\0`、**按 git 规则排序**；
   - commit：`tree`/`parent` 可解析、`author`/`committer` 行格式正确；
   - tag：`object`/`type`/`tag` 行存在且 object 存在、type 与实际类型一致。
3. **index**：trailer 的 sha1 必须与内容一致（`Index::read` 已做的话就调用它）；
   每个 stage 条目的 oid 必须在对象库里存在（loose 或 pack）。
4. **pack**：`.idx` 的对象数必须等于 `.pack` 头里的 count；每个对象都能解开并能重算出 oid
   （抽样也行，但 `--full` 必须全量）。
5. 输出对齐真实 `git fsck` 的**结论方向**（不是逐字节）：没问题时 exit 0 且不报错误；
   发现问题时报到 stderr、exit 非 0。**dangling 对象不算错误**（与 git 一致）。

### 3.2 `mg gc`
1. 把**所有 loose 对象**打成一个 `.pack` + `.idx`（放在 `objects/pack/`，文件名
   `pack-<pack 内容的 sha1>.pack` / `.idx`，与 git 的命名一致）。
2. 写/更新 `packed-refs`（格式：`# pack-refs with: peeled fully-peeled sorted`，
   然后 `<oid> <refname>` 每行一条，按 refname 排序；**annotated tag 的 `^` 行可选**，
   不做就在 result 里说明）。
3. 删掉**已被打包**的 loose 对象（保留未被引用的 dangling 对象也可，说明即可）。
4. 幂等：连续跑两次 `mg gc` 不应破坏仓库；第二次要么无事发生、要么产生等价结果。
5. 硬标准（最重要）：`mg gc` 之后
   - **真实 `git fsck --no-progress`** 无 error（允许 dangling）；
   - `git log --all --oneline` 与 gc 前一致；
   - `git cat-file -p <任意 oid>` 全部正常；
   - `git gc`/`git repack` 能把 mg 生成的 pack 正常读取（例如 `git verify-pack -v` 通过）。

### 3.3 全量 e2e 收口
写一组「真实仓库、真实 git」的端到端测试，覆盖 v1 声称支持的全部命令的**happy path**：
`init → hash-object → add → status → commit → log → tag → branch → switch → diff →
merge（含冲突）→ reset → checkout → clone(file://) → fetch → push → fsck → gc`。
每一步都与**真实 git** 的同一操作对拍（平行仓库或对同一仓库的真实 git 复查），
比较 `git fsck`、`git log`、`git status --porcelain`、`git ls-files --stage` 等 git 侧的输出。

## 4. 验收（必须能独立复跑）

```bash
cd /home/user/Projects/mini-git
cargo test --offline
cargo clippy --offline --all-targets   # 0 warning
scripts/check-freeze.sh                # drift 0
```
外加 3 个必须手动跑并贴原始输出的场景：
1. 真实 git 仓库（含 packfile）→ `mg fsck` 全绿；
2. `mg gc` 一个只有 loose 对象的仓库 → 真实 `git fsck` 全绿 + `git log --all` 一致 + 对象数下降；
3. **故意损坏**（改一个 loose 对象的字节 / 截断一个 pack）→ `mg fsck` 必须报错且 exit 非 0。

## 5. 非目标

- 不做 `git gc` 的全部行为（reflog 过期、prune 策略、cruft pack、multi-pack-index、
  delta 压缩策略）。mg 的 pack **可以不做 delta**（全量存储），但必须在 result 里说明，
  并证明真实 git 能读（`git verify-pack -v` 不报错）。
- 不改 `Cargo.toml`；需要新依赖请报 `blocked`（`sha1`/`zlib` 已有内部实现可复用）。

## 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`。
- `output`：命令 + 真实结论（git fsck 输出、对象数前后、损坏场景是否真的报错、
  e2e 覆盖了哪些命令）；已知限制/未覆盖项单列。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。要委派必须显式写 `--parent-depth 1`。

## 交互纪律（W1/W2/W3 实测总结）

- 不触发任何额外交互；omp 轮次结束后若弹「学习练习」对话框 → 选「不要」。
- 原生审批只按「单条」处理（选 1，不选 2）。
- 不 export `GIT_*`；用 `git -c key=value` / `env VAR=... git ...`。
- 共享 checkout：本 wave 另有 T13/T14 在改 `src/transport/**` 与 `src/cli/{clone,fetch,push,pull}.rs`；
  编译错误若出自那些文件，注明「不可归因于本任务」等 30 秒重试。
- 变异测试/临时构建用独立 `CARGO_TARGET_DIR` 或独立副本；临时仓库放 `/tmp/<你的名字>/`。

## 附：前提可被质疑（W3 教训，必读）

本任务书里的**任何**「真实 git 会 X」的断言都只是 controller 的提示，不保证完整：
W3/T11b 实测到一个反例 —— 同一句「写路径遇 symlink 祖先时 git 放行」在夹具不同（顺 symlink 能否
`stat` 到目标）时结论**相反**。因此：

- 动手前先自己用真实 git 复跑一遍任务书里的关键前提；若与实测不符，**先报冲突（附原始命令与输出），
  再按实测的真值实现**，不要为了「照做」而写出与 git 不一致的代码；
- 反过来，如果本任务书的**验收标准**与真实 git 冲突，以真实 git 为准，并在 result 里写明冲突点。

## 附：测试不得「skip 即通过」

任何测试用例都不允许在环境缺失（`git` 不在、网络不可达等）时 `return` 成**通过**：
要么 `panic!`/`assert!` 硬失败，要么用 `#[ignore]` 显式标出（W3/V9 在 T9 的作者用例里发现过这种假绿）。
