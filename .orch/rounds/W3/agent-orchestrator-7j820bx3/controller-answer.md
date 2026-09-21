# T12 blocked 的裁决（controller round，2026-09-19 21:12 +08:00）

T12（hermes）报 `blocked`，理由是「两个红 target 都在白名单外，且都是断言 T12 之前行为的过期测试」。
**裁决：不是 T12 的缺陷，由 controller 修，T12 不需要返工轮。**

| 问题 | 裁决 | 证据 |
|---|---|---|
| `tests/interop/smoke.rs:130` 断言「`mg status` 必须报 not implemented」 | 这是 W0 的脚手架用例，controller 已改写为「已实现命令要像 git + 未知子命令必须响亮失败」 | `ORCHESTRATION.md` C-18；`FREEZE-v0.md` v0.5（smoke 哈希 `e8d25bff98adbb4e` → `26dd76b1eaec966e`）|
| `tests/verify_worktree.rs:886 deviation_type_change_regular_to_symlink` 断言类型变化渲染成 ` M` | 这是 W1 验证者记录的**已知偏差**；C-13 加 `TypeChanged` + T12 真正产出后偏差消失，controller 已改成断言与 git 逐字节一致（改名 `type_change_regular_to_symlink_matches_git`） | `ORCHESTRATION.md` C-19；controller 复跑该 target 25/25 绿 |

**对 T12 的评价**：作者没有为了「让门禁变绿」去动白名单外的文件，而是带着最小修法上报 —— 这正是
任务书 §2 与 C2/S9 想要的行为。工作完成度按 `success` 记账（详见 W3 复盘）。

修完后的 controller 门禁：`cargo test --offline` 全绿（lib 234 + interop 7 + verify 3 + verify_diff 16(+1 ignored)
+ verify_diff_paths 6 + verify_index 11 + verify_mergebase 10 + verify_odb 6 + verify_pktline 10 + verify_refs 8
+ verify_worktree 25 + verify_plumbing 9 等，0 failed）；`clippy --all-targets` 0 warning；
`scripts/check-freeze.sh` → `21 file(s), drift 0`。
