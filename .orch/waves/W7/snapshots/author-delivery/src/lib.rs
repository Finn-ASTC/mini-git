//! mini-git —— 一个与真实 git 互操作的 git 子集实现。
//!
//! # 文件所有权（多 agent 并行协作的写作用域规则）
//!
//! * **CONTROLLER-OWNED**（child agent 不得修改）：`Cargo.toml`、`src/main.rs`、
//!   `src/lib.rs`、各 `src/*/mod.rs` 中的 `pub` 声明区、`tests/interop/**`。
//! * **MODULE-OWNED**：每个子模块的实现文件属于该模块的负责 agent。
//! * 公共签名改动 = controller round（见 `ORCHESTRATION.md` S9）。
//!
//! # 分层规则
//!
//! `oid` / `zlib` / `error` / `repo` 是 L0 基础层，不认识路径语义；
//! `object` / `index` / `refs` / `worktree` 是 L1；`odb` / `diff` / `merge` / `transport` 是 L2–L3；
//! `cli` 只做参数解析与编排，不直接读写文件。

#![forbid(unsafe_code)]

pub mod cli;
pub mod diff;
pub mod error;
pub mod index;
pub mod merge;
pub mod object;
pub mod odb;
pub mod oid;
pub mod refs;
pub mod repo;
pub mod transport;
pub mod worktree;
pub mod zlib;

pub use error::{Error, Result};
pub use oid::Oid;
pub use repo::Repo;
