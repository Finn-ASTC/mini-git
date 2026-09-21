# 本轮的 controller 决策记录（原生 UI，不是协议 blocked）

## D1（T1 / omp）：学习练习弹窗 —— 拒绝
- 现象：omp 在**已经发布 result.json 之后**弹出 `Ask` 对话框：
  「Would you like a quick 10-15 minute learning exercise on T1 的 git 字节格式决策…」，
  选项 `Yes, do the exercise (Recommended)` / `No, skip it` / `Other`。
  同时 herdr 状态变为 `blocked`，终端标题变成 `π !`。
- 判定：这是 omp 的产品级功能询问（不是权限、范围或安全审批），属于既有授权内的例行选择。
- 动作：读完整弹窗 → 光标由 Yes 下移到 `No, skip it` → Enter。
  omp 回复 "Understood — no learning exercise this session."，回到 `idle`（标题 `π >`）。
- 影响：轮次不受影响（result 已发布且验证通过）；但它把目标停在 `blocked` 状态，
  若 controller 不在监督，会被误判成「协议级 blocked」（那是需要新轮次的另一种情况）。
- 价值：跨 kind 行为差异证据 —— 作者 kind 会在轮次结束后主动弹产品对话框，需要 controller 处理。

## D2（V1 / codex）：原生命令审批 —— 逐条批准
- 现象：codex 执行任务包里的 scratch 仓库命令时弹出原生审批：
  「Bash(<compound command>) 1. Yes, proceed (y) / 2. Yes, and don't ask again for commands that start with `…` (p) / 3. No, and tell Codex what to do differently (esc)」。
- 判定：命令内容与任务包 §2 授权的 `verify-scratch/` 目录完全一致（git init/add/write-tree/cat-file/tag），
  属于既有授权内的例行确认，不需要用户决定。
- 动作：读实际命令 → 确认光标在 `1. Yes, proceed (y)` → Enter。
  **未选择 `2. Yes, and don't ask again…`**（不做批量授权，不扩大自动批准范围）。
- 观察：codex 对复合 bash 命令逐条询问，因此同一次验证会出现多次审批弹窗；
  若选择 2 可以显著减少往返，但那会把「一条命令前缀」变成常驻授权，故本轮保持逐条批准。
