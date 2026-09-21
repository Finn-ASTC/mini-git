//! 引用广告与 fetch/push 协商。**T13/T14（codex / opencode）实现范围**。
//!
//! fetch 流程（协议 v0）：
//! 1. 读 `refs` 广告：第一行形如 `<oid> <name>\0<capabilities>`，其余行 `<oid> <name>`，
//!    以 flush-pkt 结束；`symref=HEAD:refs/heads/main` 与 `side-band-64k` 从 capability 里取。
//! 2. 发送 `want <oid> <caps>\n` × n、`flush`、`have <oid>\n` × n、`done\n`。
//! 3. 收 `NAK` / `ACK <oid>`，然后 `side-band` 里是 pack 数据。
//!
//! push 流程：发送 `<old> <new> <ref>\0<caps>`（首行带 caps）+ flush，然后收
//! `report-status` 的 `unpack ok` / `ok <ref>` / `ng <ref> <reason>`。
//!
//! 验收：能从本地 `git http-backend` 或 `file://` 远程拉到真实 pack 并写出正确对象。

use crate::error::{todo, Result};
use crate::oid::Oid;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefAdvertisement {
    pub refs: Vec<(String, Oid)>,
    pub capabilities: Vec<String>,
    /// 形如 `("HEAD", "refs/heads/main")`。
    pub symrefs: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct FetchRequest {
    pub wants: Vec<Oid>,
    pub haves: Vec<Oid>,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushCommand {
    pub old: Oid,
    pub new: Oid,
    pub name: String,
}

pub fn parse_advertisement(_payload: &[u8]) -> Result<RefAdvertisement> {
    todo("transport::negotiate::parse_advertisement (T13/T14)")
}

pub fn build_fetch_request(_req: &FetchRequest, _caps: &[&str]) -> Vec<u8> {
    todo!("transport::negotiate::build_fetch_request (T13/T14)")
}

pub fn build_push_update(_cmds: &[PushCommand], _caps: &[&str]) -> Result<Vec<u8>> {
    todo("transport::negotiate::build_push_update (T13/T14)")
}

/// 解析 `report-status`，返回 `(ref, status)` 列表；`unpack` 失败也要能看到。
pub fn parse_report_status(_payload: &[u8]) -> Result<Vec<(String, String)>> {
    todo("transport::negotiate::parse_report_status (T13/T14)")
}
