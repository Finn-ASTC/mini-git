//! smart HTTP 传输。**T14（opencode）实现范围**。
//!
//! 端点：`GET <url>/info/refs?service=git-upload-pack`（fetch）
//! 与 `GET <url>/info/refs?service=git-receive-pack`（push），
//! 之后 `POST <url>/git-upload-pack` / `POST <url>/git-receive-pack`。
//!
//! 设计约束（v1）：只用 `std::net::TcpStream` 实现 **明文 HTTP/1.1**，
//! 目标是本机起的 `git http-backend`；`https://` 报 `Unsupported`（不引 TLS 依赖）。
//! 请求头最少需要：`Content-Type: application/x-git-upload-pack-request`、
//! `Accept`、`Content-Length`、`User-Agent`；响应要按 `Content-Length` 或
//! chunked 解码，并跳过可能出现的 `# service=...` 服务行与 flush-pkt。

use crate::error::{todo, Result};

pub struct HttpRemote {
    pub url: String,
}

impl HttpRemote {
    pub fn new(url: &str) -> Self {
        HttpRemote {
            url: url.trim_end_matches('/').to_string(),
        }
    }

    pub fn info_refs(&self, _service: &str) -> Result<Vec<u8>> {
        todo("transport::http::HttpRemote::info_refs (T14)")
    }

    pub fn upload_pack(&self, _body: &[u8]) -> Result<Vec<u8>> {
        todo("transport::http::HttpRemote::upload_pack (T14)")
    }

    pub fn receive_pack(&self, _body: &[u8]) -> Result<Vec<u8>> {
        todo("transport::http::HttpRemote::receive_pack (T14)")
    }
}
