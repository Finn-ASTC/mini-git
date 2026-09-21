# W0/W1 接口冻结清单（freeze-v0，v0.8 修订）

生成时间：2026-09-19T19:27:29+08:00

## 修订历史

- **v0.8**（2026-09-21，发布前）：三处**非接口**改动，**公共签名零改动**。
  ① `Cargo.toml` 增加 `repository` 元数据（发布用；哈希随之从 `dc9bcad30489f47a` 变为 `0d975c6706f85444`）；
  ② 修掉 `mg cat-file -p <tree>` 的一处**字节级兼容缺陷**：tree 条目的文件名直接落原名，漏了
  git 的路径引号规则 → 含 `"` / `\` / 制表符 / 非 ASCII 的名字与 `git ls-tree` 不一致
  （`status` 与 `diff` 早已实现该规则，只有 cat-file 漏了）。改为复用 `cli::diff::quote_path`
  （该函数提为 `pub(crate)`，规则仍然只有一份），并把「需要引号的名字」加进 `cli::cat_file`
  的 ls-tree 对拍 fixture —— 负对照实测：旧实现 FAIL（`differs from git ls-tree`），新实现 PASS。
  ③ `src/transport/local.rs` 的 git 真值 helper 钉上 `LC_ALL=C`：宿主 `LC_MESSAGES=zh_CN.UTF-8`
  时 git 打印中文，而两个用例逐字节比较 git 的英文 stderr → 假红 2 例（562 → 560 passed）。
  证据：dev 与 release 各 562 passed / 0 failed / 1 ignored、`clippy` 0 warning、`fmt --check` 干净、
  200 例随机语料 release == debug == 真实 git；`scripts/check-freeze.sh` → drift 0。
- **v0.0**（2026-09-19 19:09）：W0 骨架首次冻结。
- **v0.1**（2026-09-19）：把桩体从 controller 独占的 `mod.rs` 搬进各自模块的实现文件
  （新增 `src/refs/store.rs`、`src/worktree/status.rs`、`src/odb/pack/set.rs`），
  使 **child agent 永远不需要编辑冻结文件**。**公共签名零改动**（纯搬移），
  证据：`cargo test` 39 项全绿、`clippy` 0 warning。
  原因：W0 的布局让 T2/T3/T4 必须改 `refs/mod.rs` 等文件，违反 C1 写作用域互斥；
  这是 controller 侧的设计缺陷修复（记录于 ORCHESTRATION.md 决策记录）。
- **v0.7**（2026-09-19，W4 收口）：全仓 `cargo fmt --all` 统一格式（agent 交付的 47 个文件里
  只有 1 个是冻结文件：`src/odb/pack/mod.rs`，**纯空白改动**）。**公共 API 零改动**，
  `cargo fmt --all -- --check` exit 0。证据：`cargo test --offline --no-fail-fast` → 562 passed / 0 failed。
  原因：W0 的验收就把「fmt 干净」列为质量门，而 agent 交付的代码只跑过 `clippy`/`test`，没人跑 `cargo fmt`。
- **v0.6**（2026-09-19，W4 收口）：删除 `src/error.rs` 的 W0 桩函数 `pub(crate) fn todo<T>()`
  —— 所有里程碑实现完成后它是死代码（clippy `dead_code` 常驻 1 warning）。**公共 API 零改动**
  （`pub(crate)` 内部工具，全仓无引用，`Error::NotImplemented` 保留）。`src/error.rs` → `1b9fda525eaf58f7`。
  证据：`cargo clippy --offline --all-targets` 全仓 0 warning；`cargo test --offline` 全绿。
- **v0.5**（2026-09-19，W3 中途）：controller 修掉两个**断言的是旧世界**的测试
  （都不是公共接口，`src/**` 一字未改）：① `tests/interop/smoke.rs` 的
  `unimplemented_commands_fail_loudly_instead_of_pretending` 断言「`mg status` 必须报 not implemented」，
  而 `mg status` 已在 W3/T12 实现 → 换成两条长期有效的契约（已实现命令要像 git；未知子命令必须响亮失败），
  `c8...` → `26dd76b1eaec966e`；② `tests/verify_worktree.rs::deviation_type_change_regular_to_symlink` 是 W1 验证者
  记录的**已知偏差**（那时 `ChangeKind` 没有 `TypeChanged`，mg 只能输出 `M`），C-13 加变体 + T12 真正产出后
  偏差消失 → 改成断言与 git 逐字节一致（改名 `type_change_regular_to_symlink_matches_git`）。
  原因与证据：`ORCHESTRATION.md` C-18 / C-19。
- **v0.4**（2026-09-19，W2 开工前）：两处 controller 侧接口修正（都是 W1 验证者用真值对拍发现的）——
  ① `FileMode::from_bytes/from_u32` 对 gitlink（`160000`）返回 `Error::Unsupported` 而不是 `Corrupt`
  （`src/object/mod.rs` → `7fc40c839298f6b4`）；
  ② `ChangeKind` 增加 `TypeChanged`（`code()` → `'T'`），对齐 git porcelain 的类型变化列
  （`src/worktree/mod.rs` → `b115fff8b987ed20`）。
  两条都不改任何已有签名；②需要 T4b 跟进轮次在 `status.rs` 里真正使用该变体。
- **v0.3**（2026-09-19，W1 收尾后）：controller 修掉 `src/cli/mod.rs` 里的一个**panic 级**缺陷
  （全局 repeatable `-v` 与 `Command::Add` 的同名 `-v/--verbose: bool` 冲突，clap 在 parse 阶段 panic：
  `Mismatch between definition and access of verbose`）。改法：`Add` 不再单独声明该参数，
  dispatch 用 `cli.verbose > 0` 传入，`add::run(pathspec, bool)` **签名不变**（对 T12 无影响）。
  新哈希 `d01468899efd21c1`。证据：修复前 `mg add a.txt` exit=101；修复后 exit=1 且打印
  `not implemented yet: cli::add (T12)`（详见 ORCHESTRATION.md C-11）。
- **v0.2**（2026-09-19，W1 全量并行前）：在 `src/worktree/mod.rs` 的模块文档里**补充说明**
  （不动任何签名）：`Worktree::status` 的 `head_tree` 是**已展平**的 tree
  （entry 的 `name` 是仓库相对全路径）。原因：展平需要 odb（W2/T5 才有），
  若不澄清，T4（W1）会隐式依赖尚未实现的 odb；澄清后 `worktree` 与 `odb` 解耦，
  T4 可用 `git ls-tree -r` 作为唯一真值独立测试。
  同时把 `src/worktree/materialize.rs` 从 T4 移出（依赖 odb，归 W3/T11）。

## 规则

标为 CONTROLLER-OWNED 的文件只能由 controller 修改；child agent 需要改 = 走 controller round（S9）。

| 文件 | sha256（前 16 位） |
|---|---|
| `Cargo.toml` | `0d975c6706f85444` |
| `Cargo.lock` | `c256d3a1975ff598` |
| `src/lib.rs` | `69034fc618d4da72` |
| `src/main.rs` | `4a32188fa25bb7eb` |
| `src/oid.rs` | `9e451dee3522fcce` |
| `src/zlib.rs` | `0b37829789a78287` |
| `src/error.rs` | `1b9fda525eaf58f7` |
| `src/repo.rs` | `201bce6c1181f584` |
| `src/object/mod.rs` | `7fc40c839298f6b4` |
| `src/index/mod.rs` | `cb19ae5281fec131` |
| `src/refs/mod.rs` | `9fc4eef1c3153452` |
| `src/worktree/mod.rs` | `b115fff8b987ed20` |
| `src/odb/mod.rs` | `6c1ba463d8e5ae94` |
| `src/odb/pack/mod.rs` | `bc2af0e699fb50c6` |
| `src/diff/mod.rs` | `9ddfc8c620aa7538` |
| `src/merge/mod.rs` | `f440ddb0197fec6c` |
| `src/transport/mod.rs` | `f3640f7c7571af83` |
| `src/cli/mod.rs` | `d01468899efd21c1` |
| `tests/interop/main.rs` | `c21bd57b0bc09c1e` |
| `tests/interop/common/mod.rs` | `ef6f4d6dd086b458` |
| `tests/interop/smoke.rs` | `26dd76b1eaec966e` |

## 每个 wave 的开工快照

`FREEZE-v0.md` 管的是**公共接口**是否被越权改动；判断「某个 wave 改了哪些文件」用
`scripts/snapshot.sh` + `scripts/drift.sh`，快照存成 `.orch/waves/W<n>/baseline-freeze.txt`。
