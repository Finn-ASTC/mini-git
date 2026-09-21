//! 差分测试套件的入口（`tests/interop/` 是一个测试 target，名字就叫 `interop`）。
//!
//! 目录结构：
//! * `common/` —— verifier-only 的公共工具（临时仓库、隔离的 git/mg 调用、差分断言）。
//! * `smoke.rs` —— W0 冒烟用例；后续每个 wave 的实现型验收用例按模块加文件，
//!   例如 `object.rs` / `index.rs` / `status.rs` / `merge.rs`，在此声明即可。

mod common;
mod smoke;
