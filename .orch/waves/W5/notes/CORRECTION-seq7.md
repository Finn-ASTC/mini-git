更正：`.orch/waves/W5/notes/seq7-result-review.txt` 里我写了
「磁盘上存在 Cargo.lock，但 result 的 files_created 未列出它 → 声明不完整」，
**这条判断是错的**：作者把 Cargo.lock 声明在 `files_generated` 里
（见快照 snapshot.json 的 declarations：files_generated=['Cargo.lock']，
以及作者输出结尾的 “Files: created Cargo.toml, src/main.rs, tests/cli.rs; generated Cargo.lock”）。
我只是看了 `files_created` 一个字段就下了结论，没有看完整的四个文件字段。

处置：原 note 是已登记的回执的一部分，按「已发布记录不追改」的原则**保持原样**，
在这里另立更正。这也是本轮的一条控制方失误（记为 W5-S2）：判定文件声明完整性时
必须同时看 files_created / files_modified / files_generated / files_deleted 四个字段。
