# Controller 裁决（2026-09-19，W4/C-28）：T13c 的 blocked

你报的 blocked **成立且是本 wave 的第 4 例「测试所有权」问题**（P17/P20），不是你的实现问题。

## 裁决

- Q1 的答案就是 **(a) 跟随真实 git**（已在任务书里给定），你的实现正确；
- `tests/verify_transport.rs`（V13 的文件）里那条钉住旧行为的断言（第 962–972 行，
  `mg_fail(...)` + `assert!(!dst.exists())`）**由 controller 更新**，不授权任何人改 V13 的文件。

已完成的改动（controller 直接编辑）：
- 空远端夹具改为「与真实 git 同向」：跑一份真实 `git clone` 作对照，再跑 `mg clone`；
- 断言：`mg` exit 0 且 stderr 含 `You appear to have cloned an empty repository`；
  目标目录与 `.git` 都存在；`symbolic-ref HEAD`、`show-ref`（空）、`status --porcelain`（空）
  与真实 git 逐字段一致。

复跑：`cargo test --offline --test verify_transport` → **10 passed / 0 failed**。
controller 另外用真实仓库复跑了你 task 里的最小夹具（空裸库）：
`mg clone file://…/b.git` → exit 0 + 同一 warning；`symbolic-ref HEAD` = `refs/heads/main`；
`show-ref` 与真实 `git clone` 的空输出一致。

## 记账

- T13c 记为**一次返工轮 + 一次 controller round（C-28）**；你的 result 的 `blocked` 归入
  P17/P20（测试所有权/过期断言），不计入实现质量指标。
