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
//!
//! ## 与任务书提示的两处偏差（实测为准，见 result 报告）
//!
//! * **协议版本不由 `User-Agent` 决定**。服务端按 HTTP 头 `Git-Protocol`
//!   （CGI 里的 `GIT_PROTOCOL`）选 v2，我们不发送该头，因此 `git http-backend`
//!   稳定走 v0。`User-Agent` 仍设置成 `git/2.x` 形态，仅作兼容/诊断用途。
//! * **真实 `git http-backend` 不输出 `Content-Length`，也不接受 chunked 请求体**。
//!   CGI 输出靠外层 web server 补 framing；本实现要求响应必须带
//!   `Content-Length` 或 `Transfer-Encoding: chunked`，否则报 `Protocol` 错误
//!   （这也是任务书列出的反例之一）。
//!
//! 已知限制（v1）：不做 `https://`、HTTP/2、代理/认证（不发 `Authorization`）、
//! protocol v2、keep-alive（固定 `Connection: close`）。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::transport::pktline::{read_pkt, Pkt};

/// `git` 客户端常见的 `User-Agent` 形态。服务端不会据此选协议版本
/// （那由 `Git-Protocol` 头决定，我们刻意不发），这里只是保持与 git 一致的形态。
const USER_AGENT: &str = "git/2.55.0";

const DEFAULT_HTTP_PORT: u16 = 80;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// 非 200 响应里回显的 body 前缀长度。
const ERROR_SNIPPET_LEN: usize = 256;

pub struct HttpRemote {
    pub url: String,
}

impl HttpRemote {
    pub fn new(url: &str) -> Self {
        HttpRemote {
            url: url.trim_end_matches('/').to_string(),
        }
    }

    /// `GET <base>/info/refs?service=<service>`，返回 **跳过 `# service=...`
    /// 服务行与紧随其后的 flush-pkt 之后**的 advertisement 字节
    /// （可直接交给 `negotiate::parse_advertisement`）。
    pub fn info_refs(&self, service: &str) -> Result<Vec<u8>> {
        let service = validate_service(service)?;
        let target = format!("{}/info/refs?service={service}", self.base_path()?);
        let accept = format!("application/x-git-{service}-advertisement");
        let body = self.request("GET", &target, None, Some(&accept), None)?;
        strip_service_line(body)
    }

    /// `POST <base>/git-upload-pack`，返回纯 pkt-line 响应。
    pub fn upload_pack(&self, body: &[u8]) -> Result<Vec<u8>> {
        let target = format!("{}/git-upload-pack", self.base_path()?);
        self.request(
            "POST",
            &target,
            Some("application/x-git-upload-pack-request"),
            Some("application/x-git-upload-pack-result"),
            Some(body),
        )
    }

    /// `POST <base>/git-receive-pack`，返回纯 pkt-line 响应（report-status）。
    pub fn receive_pack(&self, body: &[u8]) -> Result<Vec<u8>> {
        let target = format!("{}/git-receive-pack", self.base_path()?);
        self.request(
            "POST",
            &target,
            Some("application/x-git-receive-pack-request"),
            Some("application/x-git-receive-pack-result"),
            Some(body),
        )
    }

    fn base_path(&self) -> Result<String> {
        Ok(parse_url(&self.url)?.path)
    }

    fn request(
        &self,
        method: &str,
        target: &str,
        content_type: Option<&str>,
        accept: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let parsed = parse_url(&self.url)?;
        let stream = TcpStream::connect((parsed.host.as_str(), parsed.port)).map_err(|err| {
            Error::Other(format!(
                "cannot connect to {}:{} ({err})",
                parsed.host, parsed.port
            ))
        })?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let mut reader = BufReader::new(stream);

        let mut request = Vec::new();
        request.extend_from_slice(format!("{method} {target} HTTP/1.1\r\n").as_bytes());
        request.extend_from_slice(format!("Host: {}\r\n", parsed.host_header).as_bytes());
        request.extend_from_slice(format!("User-Agent: {USER_AGENT}\r\n").as_bytes());
        request.extend_from_slice(b"Connection: close\r\n");
        if let Some(accept) = accept {
            request.extend_from_slice(format!("Accept: {accept}\r\n").as_bytes());
        }
        if let Some(content_type) = content_type {
            let payload = body.unwrap_or(&[]);
            request.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
            request.extend_from_slice(format!("Content-Length: {}\r\n\r\n", payload.len()).as_bytes());
            request.extend_from_slice(payload);
        } else {
            request.extend_from_slice(b"\r\n");
        }

        reader.get_mut().write_all(&request)?;
        reader.get_mut().flush()?;
        read_response(&mut reader)
    }
}

/// 只允许 git smart HTTP 的两个服务名，避免把任意字符串拼进 URL/请求头。
fn validate_service(service: &str) -> Result<&str> {
    match service {
        "git-upload-pack" | "git-receive-pack" => Ok(service),
        other => Err(Error::Other(format!(
            "unsupported smart HTTP service: {other:?} (expected git-upload-pack or git-receive-pack)"
        ))),
    }
}

/// 跳过 `info/refs` 响应开头的 `# service=...\n` pkt 与其后的 flush-pkt。
///
/// 若响应不是以服务行开头（例如 dumb 协议或已是裸 advertisement），原样返回，
/// 由调用方的 `parse_advertisement` 给出后续错误。
fn strip_service_line(body: Vec<u8>) -> Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(body.as_slice());
    match read_pkt(&mut cursor) {
        Ok(Pkt::Data(data)) if data.starts_with(b"# service=") => {
            match read_pkt(&mut cursor) {
                Ok(Pkt::Flush) => {}
                Ok(other) => {
                    return Err(Error::Protocol(format!(
                        "expected a flush-pkt after the `# service=` line, got {other:?}"
                    )))
                }
                Err(err) => return Err(err),
            }
            let consumed = cursor.position() as usize;
            Ok(body[consumed..].to_vec())
        }
        _ => Ok(body),
    }
}

struct UrlParts {
    host: String,
    host_header: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> Result<UrlParts> {
    let rest = if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else if url.starts_with("https://") {
        return Err(Error::Unsupported(
            "https:// URLs (TLS is not linked into mini-git v1)",
        ));
    } else {
        return Err(Error::Other(format!(
            "unsupported URL (expected an http:// URL): {url}"
        )));
    };

    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    // 认证不实现；若 URL 里带了 userinfo，忽略它并记在已知限制里。
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        return Err(Error::Other(format!("URL has an empty host: {url}")));
    }

    let (host, port, explicit_port) = split_host_port(authority)?;
    if host.is_empty() {
        return Err(Error::Other(format!("URL has an empty host: {url}")));
    }
    let host_header = if explicit_port || port != DEFAULT_HTTP_PORT {
        format!("{host}:{port}")
    } else {
        host.to_string()
    };
    Ok(UrlParts {
        host: host.to_string(),
        host_header,
        port,
        path: path.to_string(),
    })
}

/// 把 authority 拆成 `(host, port, port_was_explicit)`；支持 `[::1]:8080` 形式。
fn split_host_port(authority: &str) -> Result<(&str, u16, bool)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let close = rest.find(']').ok_or_else(|| {
            Error::Other(format!("malformed bracketed host in URL: {authority}"))
        })?;
        let host = &rest[..close];
        let tail = &rest[close + 1..];
        let (port, explicit) = if let Some(port_str) = tail.strip_prefix(':') {
            (parse_port(port_str)?, true)
        } else if tail.is_empty() {
            (DEFAULT_HTTP_PORT, false)
        } else {
            return Err(Error::Other(format!(
                "malformed bracketed host in URL: {authority}"
            )));
        };
        return Ok((host, port, explicit));
    }

    match authority.rsplit_once(':') {
        Some((host, port_str)) => {
            if port_str.is_empty() {
                return Err(Error::Other(format!(
                    "malformed URL authority (empty port): {authority}"
                )));
            }
            Ok((host, parse_port(port_str)?, true))
        }
        None => Ok((authority, DEFAULT_HTTP_PORT, false)),
    }
}

fn parse_port(port: &str) -> Result<u16> {
    port.parse::<u16>()
        .map_err(|_| Error::Other(format!("invalid port in URL: {port:?}")))
}

fn read_response<R: BufRead>(reader: &mut R) -> Result<Vec<u8>> {
    let status_line = read_http_line(reader, "status line")?;
    let (code, reason) = parse_status_line(&status_line)?;

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let line = read_http_line(reader, "header")?;
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                let parsed = value.trim().parse::<usize>().map_err(|_| {
                    Error::Protocol(format!("invalid Content-Length header: {value:?}"))
                })?;
                content_length = Some(parsed);
            } else if name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }

    let body = if chunked {
        read_chunked_body(reader)?
    } else if let Some(length) = content_length {
        let mut body = vec![0u8; length];
        read_exact_http(reader, &mut body)?;
        body
    } else {
        return Err(Error::Protocol(
            "HTTP response has neither Content-Length nor a chunked Transfer-Encoding".to_string(),
        ));
    };

    if code != 200 {
        return Err(Error::Other(format!(
            "HTTP {code} {reason}: {}",
            body_snippet(&body)
        )));
    }
    Ok(body)
}

fn parse_status_line(line: &str) -> Result<(u16, String)> {
    let mut parts = line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/") {
        return Err(Error::Protocol(format!(
            "not an HTTP response (first line was {line:?})"
        )));
    }
    let code = parts
        .next()
        .unwrap_or("")
        .parse::<u16>()
        .map_err(|_| Error::Protocol(format!("malformed HTTP status line: {line:?}")))?;
    let reason = parts.next().unwrap_or("").to_string();
    Ok((code, reason))
}

fn read_http_line<R: BufRead>(reader: &mut R, what: &str) -> Result<String> {
    let mut buf = Vec::new();
    let read = reader
        .read_until(b'\n', &mut buf)
        .map_err(|err| Error::Other(format!("reading HTTP {what}: {err}")))?;
    if read == 0 {
        return Err(Error::Protocol(format!(
            "connection closed while reading the HTTP {what}"
        )));
    }
    while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
        buf.pop();
    }
    String::from_utf8(buf)
        .map_err(|_| Error::Protocol(format!("HTTP {what} is not valid UTF-8")))
}

fn read_exact_http<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<()> {
    reader.read_exact(buf).map_err(|err| match err.kind() {
        std::io::ErrorKind::UnexpectedEof => Error::Protocol(
            "connection closed before the full HTTP body was received".to_string(),
        ),
        _ => Error::Other(format!("reading HTTP body: {err}")),
    })
}

fn read_chunked_body<R: BufRead>(reader: &mut R) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let size_line = read_http_line(reader, "chunk size")?;
        let size_token = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_token, 16)
            .map_err(|_| Error::Protocol(format!("invalid chunk size: {size_line:?}")))?;
        if size == 0 {
            // 消耗 trailer，直到空行。
            loop {
                if read_http_line(reader, "chunk trailer")?.is_empty() {
                    break;
                }
            }
            break;
        }
        let start = body.len();
        body.resize(start + size, 0);
        read_exact_http(reader, &mut body[start..])?;
        let mut crlf = [0u8; 2];
        read_exact_http(reader, &mut crlf)?;
        if &crlf != b"\r\n" {
            return Err(Error::Protocol(
                "chunk data is not terminated by CRLF".to_string(),
            ));
        }
    }
    Ok(body)
}

fn body_snippet(body: &[u8]) -> String {
    let end = body.len().min(ERROR_SNIPPET_LEN);
    String::from_utf8_lossy(&body[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_http_urls() {
        let parts = parse_url("http://127.0.0.1:8080/srv.git").unwrap();
        assert_eq!(parts.host, "127.0.0.1");
        assert_eq!(parts.port, 8080);
        assert_eq!(parts.host_header, "127.0.0.1:8080");
        assert_eq!(parts.path, "/srv.git");

        let parts = parse_url("http://example.com").unwrap();
        assert_eq!(parts.port, 80);
        assert_eq!(parts.host_header, "example.com");
        assert_eq!(parts.path, "/");

        let parts = parse_url("http://user:pw@example.com:81/a/b").unwrap();
        assert_eq!(parts.host, "example.com");
        assert_eq!(parts.port, 81);
        assert_eq!(parts.path, "/a/b");

        let parts = parse_url("http://[::1]:9000/x").unwrap();
        assert_eq!(parts.host, "::1");
        assert_eq!(parts.port, 9000);
    }

    #[test]
    fn rejects_https_and_bad_urls() {
        assert!(matches!(
            parse_url("https://example.com/x"),
            Err(Error::Unsupported(_))
        ));
        assert!(parse_url("ftp://example.com/x").is_err());
        assert!(parse_url("http://:8080/x").is_err());
        assert!(parse_url("http://host:notaport/x").is_err());
    }

    #[test]
    fn validates_only_git_services() {
        assert!(validate_service("git-upload-pack").is_ok());
        assert!(validate_service("git-receive-pack").is_ok());
        assert!(validate_service("git-evil").is_err());
        assert!(validate_service("../../x").is_err());
    }

    #[test]
    fn strips_service_line_and_flush() {
        let mut body = Vec::new();
        body.extend_from_slice(&crate::transport::pktline::encode_pkt(
            b"# service=git-upload-pack\n",
        ));
        body.extend_from_slice(b"0000");
        body.extend_from_slice(&crate::transport::pktline::encode_pkt(b"deadbeef HEAD\n"));
        body.extend_from_slice(b"0000");

        let stripped = strip_service_line(body).unwrap();
        let expected = {
            let mut want = Vec::new();
            want.extend_from_slice(&crate::transport::pktline::encode_pkt(b"deadbeef HEAD\n"));
            want.extend_from_slice(b"0000");
            want
        };
        assert_eq!(stripped, expected);
    }

    #[test]
    fn strip_service_line_is_noop_without_marker() {
        let mut body = Vec::new();
        body.extend_from_slice(&crate::transport::pktline::encode_pkt(b"deadbeef HEAD\n"));
        body.extend_from_slice(b"0000");
        assert_eq!(strip_service_line(body.clone()).unwrap(), body);
    }

    #[test]
    fn parses_status_lines() {
        assert_eq!(
            parse_status_line("HTTP/1.1 200 OK").unwrap(),
            (200, "OK".to_string())
        );
        assert_eq!(
            parse_status_line("HTTP/1.0 404 Not Found").unwrap(),
            (404, "Not Found".to_string())
        );
        assert!(parse_status_line("hello").is_err());
        assert!(parse_status_line("HTTP/1.1 abc").is_err());
    }

    #[test]
    fn decodes_chunked_bodies() {
        let raw = b"7\r\nMozilla\r\n9\r\nDeveloper\r\n7\r\nNetwork\r\n0\r\n\r\n";
        let mut reader = BufReader::new(std::io::Cursor::new(raw.as_slice()));
        assert_eq!(read_chunked_body(&mut reader).unwrap(), b"MozillaDeveloperNetwork");
    }

    #[test]
    fn rejects_chunk_without_crlf() {
        let raw = b"3\r\nabcXX0\r\n\r\n";
        let mut reader = BufReader::new(std::io::Cursor::new(raw.as_slice()));
        assert!(read_chunked_body(&mut reader).is_err());
    }
}
