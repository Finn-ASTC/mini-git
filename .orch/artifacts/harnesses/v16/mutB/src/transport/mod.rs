//! 传输层：pkt-line 分帧、引用协商、`file://` 与 smart HTTP。
//!
//! **CONTROLLER-OWNED（`pub` 声明区冻结）**；
//! `pktline.rs` 属 **T8（opencode）**，`local.rs` / `http.rs` / `negotiate.rs` 属 **T13/T14**。
//!
//! v1 范围（超出即 `Unsupported`）：
//! * 协议 v0 + `side-band-64k`；不实现 v2、不动 `push-cert`、不做 shallow/partial。
//! * `file://` 与 `http://`（本地 `git http-backend`）；**`https://` 明确报 Unsupported**，
//!   不引入 TLS 依赖（记录在决策记录里）。

pub mod http;
pub mod local;
pub mod negotiate;
pub mod pktline;

pub use pktline::{read_pkt, write_flush, write_pkt, Pkt};
