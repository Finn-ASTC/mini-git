# V15 —— 独立验证 T15（`mg fsck` / `mg gc` + 全量 e2e 收口）

你是本轮的**验证者**（kind: opencode），不是实现者。作者是 omp。
**不接受作者自述**，以真实 `git` 为唯一真值独立判定。

## 0. 被验证对象

- 任务包：`.orch/waves/W4/T15-fsck-gc/task.md`（先读）
- 作者结果：**由 controller 在 prompt 里给出的 result 路径**（只读，不得修改）
- 作者可改文件：`src/cli/{fsck,gc}.rs`、`tests/interop/fsck_gc.rs`
- 开工前基线：`.orch/waves/W4/T15-fsck-gc/baseline.txt`（由 controller 在 wave 启动时拍）

## 1. 写作用域（白名单）

- `tests/verify_fsck_gc.rs`（**新建**）
- `.orch/waves/W4/T15-fsck-gc/verify-scratch/**`
- 你的 result 路径

**禁止修改**其它任何文件：`src/**`（只读）、`Cargo.toml`、`Cargo.lock`、
作者的 `tests/interop/fsck_gc.rs`（只读）、`.orch/rounds/**`。

## 2. 必须独立执行的检查

### (A) 门禁复跑
```bash
cargo test --offline fsck
cargo test --offline
cargo clippy --offline --all-targets
scripts/check-freeze.sh
```

### (B) 独立真值测试（`tests/verify_fsck_gc.rs`，零硬编码真值）

**`mg gc` 的产物必须由真实 git 来判定**（mg 自读自写不算证据）：

1. 造一个**只有 loose 对象**的真实 git 仓库（`git repack`/`git gc` 之后 `git unpack-objects`
   或直接用小仓库多提交），`mg gc` 之后断言：
   - `git fsck --no-progress` 无 error（允许 dangling，dangling 不算错，与 git 一致）；
   - `git log --all --oneline` 与 gc 前**逐字节相同**；`git cat-file -p <每个 oid>` 全正常；
   - `git verify-pack -v <mg 生成的 .idx>` 通过（这是「mg 写的 pack 合法」的硬证据）；
   - `objects/` 下 loose 对象数**确实下降**（给出前后数字）；
   - `packed-refs` 格式：真实 git 能解析（`git show-ref`/`git for-each-ref` 结果不变）。
2. **幂等**：连续两次 `mg gc` → 第二次后 `git fsck` 仍无 error，`git log --all` 不变；
   （若第二次产生了新的 pack，说明不幂等，必须报出来）。
3. **`mg fsck` 的判定方向必须与真实 git 一致**（不是逐字节，是结论方向）：
   - 干净仓库（loose + pack 混合、含 annotated tag）→ `mg fsck` exit 0，且**真实 git fsck 也干净**；
   - **至少 6 种人工损坏**，逐个断言 `mg fsck` **exit 非 0 且报到 stderr**：
     ①loose 对象字节被改；②loose 对象被截断；③pack 文件被截断；④`.idx` 的 fanout/oid 表被改；
     ⑤tree entry 的 oid 不存在；⑥commit 的 parent oid 不存在 / `tree` 行缺失。
     每种都要同时贴**真实 `git fsck` 对同一损坏的输出**，证明「git 认为坏、mg 也认为坏」。
   - **反例的另一面**：dangling 对象（`git hash-object -w` 造一个没人引用的 blob）→
     `mg fsck` **必须 exit 0**（与 git 一致）。
4. **e2e 收口**：不信作者的 e2e，自己写一条最短路径：
   `init → hash-object → add → status → commit → log → tag → branch → switch → diff → merge(冲突) →
   reset → checkout → clone(file://) → fetch → push → fsck → gc`，
   每步与真实 git 对拍（平行仓库），比较 `git log --oneline`、`git status --porcelain`、
   `git ls-files --stage`。若某条命令本轮尚不可用（T13/T14 并行中），注明「未覆盖 + 原因」。

### (C) 不许假绿

- 抽 1 条作者的用例，说明它的真值来自真实 git（不是硬编码、不是 mg 自读自写）；
- 检查作者是否用「mg 读回自己写的 pack」当合法性证据（自洽式假绿），是则判 FAIL。
