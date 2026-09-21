//! packfile 与 pack index。**T9（codex）实现范围** —— 全项目最难的一块。
//!
//! * `idx.rs`：`.idx` v2 读写（fanout 表 + oid 表 + crc + offset 表 + 大 offset 表）。
//! * `read.rs`：`.pack` 对象头 varint 解析、`OFS_DELTA` / `REF_DELTA` 的 delta 还原。
//! * `delta.rs`：git 的 delta 格式（base size varint、target size varint、copy/insert 指令）。
//!
//! 验收：解析 `git gc` 产出（或真实 remote fetch 得到）的 pack，
//! 其中每个对象都与 `git cat-file` 输出一致；delta 链路（含多级 OFS_DELTA）必须解开。

mod set;

pub mod delta;
pub mod idx;
pub mod read;


pub use idx::PackIndex;
pub use read::PackFile;

/// 仓库中所有 pack 的集合（`.git/objects/pack/*.idx`）。
#[derive(Debug, Default)]
pub struct PackSet {
    pub indices: Vec<PackIndex>,
}

