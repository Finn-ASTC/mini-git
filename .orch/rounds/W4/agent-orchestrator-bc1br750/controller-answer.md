# Controller 裁决（2026-09-19，W4/C-24 + C-28）：V13 的两个 blocked 问题

## Q1：空远端 `mg clone` —— 裁决 **(a) 跟随真实 git**

- 你的实测（git 2.55.0：exit 0 + `warning: You appear to have cloned an empty repository.` + 只留 `.git`）
  被采信为真值；任务书 §4.3 与你的验证书 §(B)3 的「空仓库必须报错」是**旧口径**，作废。
- 处置：派 **T13c 返工轮**（codex，同 T13 作者）实现；你文件里那条钉住旧行为的断言由 controller
  在 C-28 更新为「与真实 git 同向」（跑一份真实 `git clone` 作对照产物，逐字段比对
  warning / 目标目录 / `symbolic-ref HEAD` / `show-ref` / `status --porcelain`）。
- 复跑：`cargo test --offline --test verify_transport` → **10 passed / 0 failed**；
  controller 用空裸库夹具独立复跑 `mg clone` → exit 0 + 同一 warning，与真实 git 产物一致。

## Q2：`tests/fsck_gc.rs` 里两条已恒绿的 `#[ignore]` —— 裁决「由 controller 直接摘除」

- 你报告的两条（`clone_of_an_origin_without_tags_succeeds`、`push_to_a_bare_remote_updates_and_creates_branches`）
  实跑均通过，已摘除 `#[ignore]`；同轮还摘掉了第三条
  `e2e_fast_forward_merge_in_a_repository_with_subdirectories`（T16 修好后恒绿）。
- 顺带发现并摘掉了 **W2 遗留的 `tests/verify_diff.rs::cli_space_in_path_tab_padding`**
  （V6 标记、T6b 修好后再没人摘）→ 这一条写成 skill 缺陷 **P20**（过期 `#[ignore]` 不会自己失效），
  并把「wave 收口复跑 `--ignored` 逐条对账」写进流程（`ORCHESTRATION.md` C-25）。
- 全仓 `ignored` 从 6 降到 **1**（唯一保留的是 T15 报告的已确认分歧
  `fsck_detects_corruption_in_unreachable_objects`）。

## 记账

- V13 判定的**被验证对象（T13 六文件）为 PASS**，两个 blocked 问题都与实现无关，
  属 P17/P20（测试所有权/过期标记）类；T13c 之后你不需要再复验（controller 已独立复跑关键夹具）。
