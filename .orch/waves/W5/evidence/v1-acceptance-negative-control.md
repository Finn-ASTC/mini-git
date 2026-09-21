# V1 验收补做：按新配方加负对照（2026-09-20 18:18，skill b124b87 之后）

## 为什么要补

W5 收口时我按当时能推导出的路径验收了验证者 V1：封存它的证据范围，再用**控制方自己构建的
release 制品**重跑 V1 自写的 oracle/harness，得到 36/36。**但那只跑了正例** ——
没有证明这套 oracle/harness *会拒绝错误输入*。如果 harness 是「永远 PASS」的假绿，
我的 36/36 和 V1 的 36/36 就都毫无意义（这正是 P18 的老问题：没有负对照就没有真值）。

skill 更新到 `b124b87` 后，`references/completion.md` 的 §"Accepting a verification round"
把这条明确写成要求：

> Define a controller-owned plan using their actual interface; exercise both a
> known-correct case and a deliberately incorrect one. Require a specific rejection
> outcome, not any nonzero exit that might just be a missing dependency.
> ... Reading a PASS report or running only positive cases is insufficient.

所以我在**不改动已封存记录**的前提下补做了这条，作为独立的新 attempt。

## 补做内容

新计划 `.orch/waves/W5/plans/v1-artifact-plan-v2.json`，对同一个 V1 快照
（`deliveries-v1/snapshot-e2_cmplo/snapshot.json`，未被修改）执行：

1. 断言 `oracle/linestat_oracle.py` sha256 `90d2a744…9389ae9`、
   `harness/differential.py` sha256 `cae5eada…86381d8d`、
   被测制品 sha256 `75953480…8a4f5e` —— 三者都钉死；
2. **负对照**：在 attempt 自己的可写 `build/` 里造一个故意永远答错的替身
   （`wrong_linestat.py`，固定输出 `0 0 0 total`），用同一个 harness 跑它，
   **要求被拒**；
3. **正例**：同一个 harness 跑真的 release 制品，要求 36/36。

## 结果

```
attempt: /tmp/w5-v2-attempt/verify-fw2ab0ic/attempt.json   passed: true
negative control (wrong_linestat.py, sha256 908c37c0…a728f3):
    cases_total 36, cases_passed 0, cases_failed 36   exit != 0
positive (verify-l9da_n_s release artifact, sha256 75953480…8a4f5e):
    cases_total 36, cases_passed 36, cases_failed 0    exit 0
```

负对照**被 36/36 全数拒绝**，证明这套 oracle/harness 具备判别力，
W5 原本的 36/36 PASS 因此不再可能是「永远 PASS」的假绿。

## 记录原则

- 原验收证据（`v1-acceptance.txt`、`deliveries-v1/` 下的快照与 attempt）**原样保留、不追改**；
- 本条是**补充证据**，不回头改写 `jobs.py` 里已 closed 的完成记录
  （按「已发布记录不追改」原则，后补证据不追溯变更历史结论）；
- W5 的 V1 job 收口时**缺少**这条负对照，属我当时的遗漏，已如实记入 `defects.md` / `summary.md`。
