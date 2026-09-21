# W6 —— 上游 C1/C2/B2 批的脚本级实测（2026-09-21）

**这不是一次 agent wave。** 本轮没有起子 agent、没有花模型 token：上游这一批
（`kumi-public@d90cc8d`）的能力全部落在控制器侧的读取/等待工具上，可以直接对
**真实数据**（W5 那次真跑留下的 run）取证，不需要模拟宿主。

## 0. 先解决了上一轮的疑问：仓库拓扑

| 仓库 | 状态 |
|---|---|
| `~/Projects/kumi` | **已归档**（`0ea1a17`），不再接受改动 |
| `~/Projects/kumi-public` | **权威源**，HEAD `d90cc8d` |
| `~/Projects/agent-skills-vault` | 运行包；`0d8e284` 显式记「来自 kumi-public origin/main = d90cc8d」，`6852f56` 只改 PROVENANCE/README |
| `~/.agents/skills/*` | 仍指向 vault 的软链 |

`diff -rq ~/Projects/kumi-public/skills ~/Projects/agent-skills-vault/agents` → 只差 vault 自有的 `PROVENANCE.md`。
**运行时没有陈旧，无需再同步。**

## 1. 测了什么

| 编号 | 被测能力 | 数据源 | 结论 |
|---|---|---|---|
| C1 | `runs.py recover --summary` 分页摘要 | 真实 W5 run | ✅ 与完整恢复的计数/健康一致，0.079s |
| C2 | `recover --delta` 增量续读与持久游标 | 真实 W5 run | ✅ 首读建缓存 + 全量 upsert；续读 0 变更、源读 71→1；`token_mismatch` 全量重放；非法/外地/软链游标被拒；损坏游标整份丢弃重建 |
| C2 | 副作用边界（文档称「绝不改既有 task/result/review」） | 真实 W5 run | ✅ 429 个文件的 sha256 清单：0 改 0 删，只新增 `recovery/<id>/` 3 个文件；清理后与测试前**逐字节相同** |
| B2 | `wait_output.py` 检查点等待 | 自建真实 producer | ✅ 文件尚不存在即开始等 → 0.40s 命中 CHECKPOINT_READY，**业务进程仍在运行**；超时可续；部分行/软链/FIFO/超长行/截断/替换全部按文档拒绝；零输出文件 |
| B2 | stdin 发布配方（`PUBLICATION_ARGV`） | 真实 `prepare` 生成的轮次 | ✅ 无 shell、stdin 发布一次成功、零候选文件、`validate --check-files` 通过 |
| B2 | W5-S1 回归（作者重发已发布 result） | 同一轮次 | ✅ 原 `result.json` sha256 不变，重发一律被拒（但**错误信息不可读**，见 P28） |

## 2. 新发现

- **P26 🟡 成本向**：reset 路径（`token_mismatch` / `token_required` / `version_changed`）无条件重写整份
  `contents.json`，即使内容逐字节未变。实测 203,595 B 白写；对照组的 `cache_write_bytes:0` 证明判据本身能工作，
  是 reset 分支丢了 `cache_sha256` 键导致的。详见 `SKILL-FINDINGS.md` §P26。
- **P27 🟢 文档**：`--after` 必须是行边界，否则 exit 2，文档未写。
- **P28 🟢 可用性**：拒绝重发时抛裸 `OSError`（泄漏 `.result.json.<rand>`），未说明「已存在有效响应」。
- **P29 🟢 文档**：`PUBLICATION_ARGV` 只在 `prompt.txt`，`prepare` 的 JSON 响应里没有。

## 3. 一个反直觉但正确的行为（记下来免得下次误判）

run 目录**不能搬迁**，也不能靠改路径字段来"复制一份"：

| 操作 | 结果 |
|---|---|
| `cp -a` run 到 `/tmp` 后 `recover` | `run path/temporary flag changed` |
| 只改 `run.json` 的路径字段 | `index identity/revision mismatch`（2 条）+ `round membership changed`（2 条）+ `watch target lost run membership`（2 条）+ `unregistered run artifact`（4 条）= 10 条错误 |
| 再全量替换 243 个文件里的绝对路径 | 错误降到 6 条（`request run membership changed`，因为 `request_sha256` 是原始字节的哈希） |

这是**完整性设计**（索引、request、watch 都钉了路径归属与 sha256），不是缺陷。
但它意味着「离线复制一份 run 再分析」这条路走不通 —— 只能原地读，或走 delta 这种只写 `recovery/` 的方式。

## 4. 证据

`evidence/` 下是本轮全部原始输出：8 份 delta/summary JSON、3 份 429 文件清单（测试前 / delta 后 / 清理后）。
所有命令都在**运行时包**（`~/.agents/skills/...`）上执行，没有改 skill 一个字节。
mini-git 侧 `scripts/check-freeze.sh` → `checked 21 file(s), drift 0`。

## 5. 还没做的（需要真 agent，属 W7）

- B2 的**原生宿主侧**验证：`wait_output.py` 打在宿主管理的 handle 上，看原生 wait 是否真的提前返回；
  `supervision.py` 的 `coverage[].renew_by`（60 秒余量）与 `unconfirmed_blocked` → 立即 `review_due`。
  这两项都需要一个**活着**的 observer，脚本级测不出来。
- C2 的 delta 在**多 job / 长跑**下的读成本对比（本轮 run 只有 2 job，规模太小，说明不了节省）。
