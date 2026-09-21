# W7 缺陷记录

## W7-D1（🟠 严重，产品向 / 由交付验收抓到）测试套件把证据**追加写进仓库**，破坏历史证据并使交付验收无法通过

**发现路径**：控制方对 T1 的交付做 `delivery.py verify` 时，验收在**第 3 条命令**
（`cargo test --offline`）之后中止，`source_unchanged: false`，报告的变更全是
`.orch/waves/W3/T9-pack/verify-scratch/*` 被**新增/改写**。

**根因**（`tests/verify_pack.rs:566`）：

```rust
fn scratch_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".orch/waves/W3/T9-pack/verify-scratch");
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn record(label: &str, lines: &[String]) {
    let text = format!("{}\n", lines.join("\n"));
    let path = scratch_dir().join(format!("v9-{label}.txt"));
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path) ...;
    file.write_all(text.as_bytes()) ...;
}
```

`record()` 用 **append** 模式写 `CARGO_MANIFEST_DIR/.orch/...`。于是**每跑一次
`cargo test`，就向仓库里的 W3 证据文件追加一份记录**。

**实测证据**（2026-09-21）：

| 文件 | 现状 | 说明 |
|---|---|---|
| `v9-byte-flips.txt` | 43 行 / **去重后 1 行** | 同一行被追加 43 次 → 该集成测试至少跑过 43 次 |
| `v9-delta-api.txt` | 43 行 / 去重后 1 行 | 同上 |
| `v9-multi-pack.txt` | 43 行 / 去重后 1 行 | 同上 |
| `v9-corrupt.txt` | 301 行 / 去重后 91 行 | 多行块重复 |
| `v9-delta-chains.txt` | 324,994 B | 含 1,763 个重复块标记 |

10 个 `v9-*.txt` 的 mtime 全部变成 **09-21 01:45**（作者跑门禁的时刻），而同一目录里
未被测试触碰的 `v9-final-run.log` / `v9-mutation-summary.txt` 仍是 **09-19 21:09/21:10**。
这条对照说明：**改动的确是 `cargo test` 造成的，不是人工编辑**。

**三重影响**：

1. **历史证据被污染且不可逆**：W3/V9 的证据文件不再是「当时那一份」，而且每跑一次就长一截。
   本项目「证据归档」这一整套做法在这条路径上被自己的测试拆掉了。
2. **交付验收被正确挡住**：`delivery.py verify` 的 `source_unchanged` 检查要求验收副本在跑命令期间
   不被改动。测试自己往仓库里写 → 检查必然失败 → **这个仓库在修好之前无法通过 `kind=delivery` 的验收**。
   （工具行为是对的，被抓的是产品问题。）
3. **测试非 hermetic**：测试结果依赖仓库内的历史状态；换一个干净检出跑，同一份测试会去创建
   `.orch/waves/W3/...` 目录，行为与在老检出上不同。

**修法（T2 轮的范围）**：把证据落盘位置移出仓库；默认用临时目录（尊重 `TMPDIR`），
允许用环境变量指定，路径打印到 stderr；`.orch/**` 里已有的 10 个文件**保持原样**（不清理不重写，
它们是「被污染的历史证据」这一事实本身的证据）。

**处置（已闭环，2026-09-21 02:18）**：T2 轮修掉 —— `scratch_dir()` 删除，改为
`evidence_dir()`：`MG_EVIDENCE_DIR` 优先，否则 `std::env::temp_dir()/mini-git-v9-evidence-<pid>`（尊重 `TMPDIR`），
`record()` 写盘前把实际路径打到 stderr。10 个记录点的语义不变。
控制方验收：干净副本上跑完 dev+release 全量测试后**副本逐字节未变**（`source_unchanged: true`），
默认落点是 `$TMPDIR` 下的 10 个文件（223–7,558 B，单次运行量级），`MG_EVIDENCE_DIR` 覆盖生效；
`.orch/waves/W3/T9-pack/verify-scratch/` 的 11 个 `v9-*.txt` 前后 sha256 逐一相同（**未清理、未重写**）。
证据：`.orch/waves/W7/evidence/t2-controller-review.txt`、`t2-w3-v9-hashes-{before,after}.txt`、
`/tmp/w7-deliveries/snapshot-if97pkb6/verifications/verify-kqi9webq/attempt.json`。

**顺带记录（不是缺陷）**：`src/transport/local.rs:1540` 也用 `CARGO_MANIFEST_DIR`，
但它只是「找不到就退到 `target/`」的只读兜底，不写仓库。

---

## W7-S1（🟠 严重，流程向）验证者轮被排在作者交付**之前**，等于让验证者验证一个还不存在的东西

**发生**：W7 开工时把作者轮（T1）与验证者轮（V1）**同时** prepare 并发了出去。
V1 的输入在 T1 的修复落盘前就到达了验证者 pane。

**发现的代价**：等到 V1 的 round 已经 begin（attempt 已登记）之后，控制方才意识到
「验证者必须先于作者交付到达的只有任务书，不能是交付物」——V1 被取消
（`.orch/waves/W7/evidence/v1-not-sent.txt`、job `bfbf5d1f…` close outcome=cancelled），
另建 V2 轮并把封存快照（pre-fix-src + 交付快照）作为输入。

**教训（可以机械化的部分）**：
1. 「验证」轮的输入里必须**显式包含被验证对象的位置与哈希**；如果那个位置要等另一个 agent 写，
   那么这一轮就不能与写者同时起飞 —— 依赖关系是 `author 交付 → capture 快照 → verifier 起飞`。
2. 协议本身**不阻止**这个错误：`prepare` 不检查依赖的 input 是否存在（这是对的，工具不该猜），
   但任务包模板可以加一行「本轮输入是否已存在？谁负责让它存在？」。
3. 损失可测：V1 白花的是**控制方的时间**（prepare/begin/claim/取消/取证 ≈ 20 分钟）而不是 agent 的 token
   —— 因为取消发生在 prompt 送出之前，这正是「begin 与 send 分离」设计救回来的那部分。

## W7-S2（🟠 严重，环境向）验收/验证的构建产物把 16G 的 tmpfs 撑满，最终把沙箱本身卡住

**发生**：W1–W7 的验证轮都在 `/tmp` 下建独立的 `CARGO_TARGET_DIR`（这是纪律要求的），
但没有人负责回收。`/tmp` 是 16G tmpfs，跑到 W7 时占用到 13G（80%）。

**症状链**：先是 `cat > 文件` 报 `Disk quota exceeded`（写小文件失败）；
随后**沙箱无法启动**（bubblewrap 挂载失败）—— 也就是说「工具链跑不动了」，
而当时的直觉是「skill 坏了」。

**处置**：清掉历史 wave 的构建目录（`/tmp/v16`、`/tmp/v11-target`、`/tmp/v6b-mut`、`/tmp/t13b`、
`/tmp/w6-delta`、各 `verify-*/build` 等），保留当轮验证者的工作目录与交付快照 → 回到 5.8G/9.7G 可用。

**教训**：
1. 「独立构建目录」这条纪律需要一个**对应的回收责任人**；manifest 里应当记录每个 target 目录的
   大小与生命周期（本轮 V2 自己在收尾时删掉了 5.3G 的 scratch，这是 agent 侧的好行为，但不可依赖）。
2. tmpfs 打满的表现是**间接的、误导性的**（先是无关的小写失败，再是沙箱启动失败），
   排查顺序里应当包含「先看 `df -h /tmp`」。
3. 交付/验收快照（`/tmp/w7-deliveries/**`）不能删 —— 它属于证据；可删的是可重建的 `target/`。
