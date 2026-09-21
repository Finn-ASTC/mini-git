# T9 —— packfile 读取：`.idx` v2 + 对象头 + OFS/REF delta 还原

你是本轮的**实现者**（kind: codex）。仓库 `/home/user/Projects/mini-git` 是「与真实 git 互操作」
的实现（CLI 名 `mg`）。本轮只做一件事：**把真实 git 写出的 packfile 读对**。

## 1. 目标（签名不许改）

| 文件 | 需要实现的符号 |
|---|---|
| `src/odb/pack/delta.rs` | `parse_delta_header(&[u8]) -> Result<DeltaHeader>`、`apply_delta(base, delta) -> Result<Vec<u8>>` |
| `src/odb/pack/idx.rs` | `PackIndex::open(path) -> Result<PackIndex>`（`len/is_empty/lookup/iter_oids` 已实现，不要改语义） |
| `src/odb/pack/read.rs` | `PackFile::open(path)`、`PackFile::read_at(offset) -> Result<(Kind, Vec<u8>)>`、`PackFile::oid_at(offset) -> Result<Oid>` |
| `src/odb/pack/set.rs` | `PackSet::{open, read, contains, iter_oids}` |

`src/odb/pack/mod.rs` 与 `src/odb/mod.rs` 是 **CONTROLLER-OWNED**：`Odb::read` 已经是
「先 loose 再 pack」，你**不需要也不能**改它。

## 2. 写作用域（白名单，只有这四个文件 + 你的测试目录）

- `src/odb/pack/{delta,idx,read,set}.rs`
- `tests/interop/pack.rs`（**新建**，可选；`tests/interop/main.rs` 是 CONTROLLER-OWNED，不要改）

**不许改**任何其它文件：`src/odb/pack/mod.rs`、`src/odb/mod.rs`、`src/odb/loose.rs`（T5）、
`src/repo.rs`、`Cargo.toml`、`Cargo.lock`、其它 `src/**`、别人的 `tests/**`、`.orch/**`。
需要改白名单外文件 → 报 `blocked`。

## 3. 关键语义（照着做，别自由发挥）

1. **`.idx` v2**：magic `\377tOc` + version u32be = 2；fanout[256] u32be；
   oid 表（20B × N，升序）；crc32（N×u32be）；offset 表（N×u32be，**最高位为 1 时
   低 31 位是大 offset 表的索引**）；大 offset 表（u64be）；尾部两个 sha1（pack 的 + idx 自身的）。
   `open` 必须**校验** magic/version/fanout 单调性与长度自洽；格式不对 → `Error::Corrupt`，
   不要返回部分数据。**不要**校验尾部 sha1 之外的内容哈希（成本高，交给 `mg fsck`）。
2. **对象头**：第 1 字节 bit7 续位、bit6-4 类型（1 commit / 2 tree / 3 blob / 4 tag /
   6 ofs_delta / 7 ref_delta）、bit3-0 为 size 低 4 位；后续每字节 7 bit 高位。
   类型 5 保留 → `Corrupt`。
3. **OFS_DELTA 的负偏移 varint**：`offset = byte & 0x7f; while byte & 0x80 { byte = read();
   offset = ((offset + 1) << 7) | (byte & 0x7f) }`，base 偏移 = 当前对象偏移 − offset。
   这是**最容易写错的一处**，请用真实 pack 里的多级 OFS_DELTA 链路验证（见 §4）。
4. **delta 数据**：`base_size` varint、`target_size` varint、指令流。
   `0x00` 非法 → `Corrupt`；`0x01..=0x7f` = insert 长度；`0x80..=0xff` = copy，
   bit0..bit3 决定 offset 的 4 个字节是否出现，bit4..bit6 决定 size 的 3 个字节，
   **size 缺省 = 0x10000**。apply 后必须校验：结果长度 == `target_size`，
   且 base 长度 == `base_size`；copy 区间越界 → `Corrupt`。
5. **递归解开 delta 链**：`OFS_DELTA` 的 base 在同一 pack 内（按偏移）；`REF_DELTA` 的 base
   按 oid 在本 pack 内查（`read_at` 用 δ 的 base oid → 需要能按 oid 找偏移 →
   `PackFile::open` 时**若存在同名 `.idx` 就加载它**；没有 `.idx` 时 `oid_at` 用不到，
   `REF_DELTA` 解析不了就报 `Unsupported`，**不要**返回错数据）。
6. **防御递归爆炸**：设一个 delta 链深度上限（例如 64）与一个总字节上限，超限 → `Corrupt`。
7. **`PackSet`**：
   - `open`：扫 `objects/pack/*.idx`；已实现解析就返回真实集合（**不能再报 NotImplemented**）；
     `.idx` 存在但解析失败 → `Corrupt`（不许静默跳过，那会造成「对象查不到 = ObjectNotFound」的假绿）。
   - `read(oid)`：在某个 idx 里 `lookup` 命中 → `read_at` → **重算 oid 校验**
     （`Oid::hash_object(kind, payload)` 必须等于请求的 oid，否则 `Corrupt`）。
   - `contains` / `iter_oids`：与 `git cat-file --batch-all-objects --batch-check` 的集合一致。
8. **多 pack**：`objects/pack` 下可能有多个 `.idx`（例如 `git gc` 后残留 + 新建）。
   两个 pack 都含同一 oid 时结果必须相同；**不要**假设只有一个文件。

## 4. 验收（必须能独立复跑）

```bash
cd /home/user/Projects/mini-git
cargo test --offline pack::
cargo test --offline
cargo clippy --offline --all-targets   # 0 warning
scripts/check-freeze.sh                # drift 0
```

1. **真值对拍（核心）**：临时仓库里造一批对象（文本、二进制、空文件、大文件、
   大量相似文件以**故意触发 delta**），然后 `git gc --aggressive --prune=now` 或
   `git repack -adf`。断言：
   - `PackSet::open` 成功，`iter_oids()` 与 `git cat-file --batch-all-objects --batch-check`
     的 oid 集合**完全相等**；
   - 对每个 oid：`PackSet::read` 的 `(Kind, payload)` 与
     `git cat-file <type> <oid>` **逐字节相等**；
   - **至少有一条多级 OFS_DELTA 链**：用 `git verify-pack -v <idx>` 的输出找出
     `chain length >= 2` 的 delta 对象，逐个断言解出的内容与 git 一致
     （在 `output` 里贴出 `git verify-pack -v` 的关键行作为证据）；
   - 把同一个仓库里**所有** pack 对象都跑一遍（不要只抽 1 个）。
2. **两种数据来源**：`git gc`/`git repack` 产出的 pack（策略：`--aggressive`、
   `-adf`、`--window=50 --depth=50` 至少两种配置）。
3. **`.idx` 反查**：`PackIndex::lookup` 对每个 oid 返回的偏移，
   与 `git verify-pack -v` 里该对象的偏移**一致**（大 offset 场景若造不出来，
   至少在 output 里说明「小仓库未覆盖 >2GB 偏移」）。
4. **反例（必须失败，不是 panic）**：把 `.idx` 截断/改 magic/改 version、
   把 `.pack` 截断、把某个对象的 zlib 流破坏、delta 指令里 copy 越界 →
   都必须 `Err(Corrupt)`（或 `Unsupported` 并说明），**不得** panic、不得返回部分数据。
5. **不许假绿**：测试必须真的调 `minigit::odb::pack`；真值只能来自真实 git 进程
   （`git cat-file` / `git verify-pack`）与文件系统，不许硬编码 oid 或期望字节
   （唯一例外：`.idx` 格式里的常量）。

## 5. 非目标

- **不写 pack**（不生成 `.pack`/`.idx`）——`mg gc`（W4/T15）是否需要本地重写 pack，
  由 controller 另定；本轮只读。
- 不做 multi-pack-index、`.rev` 文件、bitmap、`pack.packSizeLimit` 分片逻辑。
- 不做 **thin pack**（base 不在本 pack 里的 `REF_DELTA`）：那是 `fetch` 的
  `index-pack --fix-thin` 场景（W4/T13/T14）。遇到时报 `Unsupported` 并在
  `output` 里写明，不要猜。
- 不改 `Cargo.toml`；需要新依赖请报 `blocked`。

## 上报

- result 路径：**由 controller 的 prompt.txt 给出（绝对路径）**，不要自己猜、不要写到别处。
- 契约：`status` / `output` / `files_created|modified|deleted` / `files_generated` /
  `error` / `blocked_reason` / `completed_at`（见 agent-controlled 协议）。
- `output` 必须包含：跑了哪些命令 + **真实结论**（对象数量、逐字节是否一致、
  最长 delta 链长度、反例是否真的失败）；不要贴完整逐字稿，也不要贴大段源码。
  已知限制/未覆盖项必须单列。

## 深度与上限

- 本轮的 depth：1；`max_depth`：3。
- 要再往下委派，必须显式写 `--parent-depth 1`，不得使用 shell 变量默认值。

## 交互纪律（W1/W2 实测总结，必须遵守）

- **不要触发任何额外交互**：不启动「学习练习 / tutorial / 引导流程」，不弹 Question/Ask
  等人类输入。轮次结束的唯一标志是 result 文件写完。
- **原生审批只按「单条」处理**：出现「1 Yes / 2 don't ask again / 3 No」时选 1，**不要选 2**；
  也不要为了少弹窗把命令合并成一条巨大脚本（那会让审批粒度失效）。
- **不要 export 会被 git 读取的环境变量**（`GIT_AUTHOR_*` / `GIT_COMMITTER_*` / `GIT_CONFIG_*`）。
  用 `git -c user.name=... -c user.email=...` 或 `env VAR=... git ...` 限定作用域。
- **共享 checkout 的编译噪声**：本 wave 另有 3 个 agent 同时改别的子目录。
  若 `cargo` 报错的文件**不在你的白名单里**，那不是你的问题：等 30 秒重试；
  连续 3 次仍失败就记录进 `output` 并有界等待（必要时报 `blocked`），**不要改别人的文件**。
- **共享 `target/`**：并发 `cargo test` 会等文件锁（正常）。但**变异测试/临时构建必须用
  独立 `CARGO_TARGET_DIR` 或独立副本**，否则会污染共享缓存（W1 的 V2 踩过）。
