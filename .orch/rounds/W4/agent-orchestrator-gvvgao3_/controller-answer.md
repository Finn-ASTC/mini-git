# Controller 裁决（2026-09-19 22:5x，W4/C-27）：T14b 的 Q1 / Q2

作者（opencode）本轮 status=`blocked`，两个问题都属**controller 侧测试维护**，不是你的实现问题。
你的修复（GET `info/refs` → `Accept: */*`）经 controller 复跑确认正确，**不需要再做任何代码改动**。

## Q1：`tests/verify_http_http.rs` 的两条 characterization 断言

裁决：**(a) 由 controller 直接更新**（不派轮、也不授权你改 V14 的文件）。已完成的改动：

- `known_defect_info_refs_accept_header_is_not_the_git_mime` → 更名
  `info_refs_accept_header_matches_real_git`，断言从 `assert_ne!(ours, real)` / `contains("git-git-")`
  翻转为 `assert_eq!(ours, real)` + `assert_eq!(ours, "*/*")`（即「与真实 git 逐字节一致」）；
- `request_shape_matches_smart_http_contract`：`accept.starts_with("application/x-git-")` → `accept == "*/*"`，
  并删掉「这里的 Accept 值本身是错的」那条注释。

复跑（**必须离开受限沙箱**，因为该 target 要 bind `127.0.0.1:0`）：
`cargo test --offline --test verify_http_http` → **18 passed / 0 failed / 0 ignored**。

## Q2：`premise_http_backend_requires_dechunked_request_body` 的 locale 失败

裁决：controller 已在该用例 spawn `git http-backend` 的 `Command` 上补 `.env("LC_ALL", "C")`
（该断言比对 git 的英文 stderr，必须与 locale 解耦）。复跑同上 18/0。

## 记账

- T14b 记为**一次返工轮 + 一次 controller round（C-27）**，你的 result 里的 `blocked` 属于
  「测试所有权」类（P17/P20），不计入实现质量指标；
- 你不是唯一撞上这一条的：W3 的 T12（hermes）撞的是同一类问题（旧世界断言 + 白名单外文件），
  处置方式相同（`ORCHESTRATION.md` C-18/C-19/C-20）。
