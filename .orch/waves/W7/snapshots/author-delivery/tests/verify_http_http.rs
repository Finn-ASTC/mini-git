//! V14（codex 验证者）—— 独立验证 T14 的 smart HTTP 客户端。
//!
//! **本文件不复用作者 `tests/verify_http.rs` 的任何脚手架**：这里自带一套
//! `std::net::TcpListener` + `git http-backend` CGI 转发的 HTTP 脚手架。
//! 每次请求都真起 TCP 连接、真起 `git http-backend` 进程；`git` 只作 oracle
//! （advertisement、pack、report-status 的真值全部来自它）。
//!
//! 与被测对象的关系：被测的是 `src/transport/http.rs` 的 HTTP 层，
//! 不是 git 本身；真值断言都落在「HTTP 响应字节 → 客户端解析结果」上。
//!
//! 真值前提（本文件底部 `premise_*` 用例）用真实 git 独立复核，不采信任务书/作者自述。

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use minigit::transport::http::HttpRemote;
use minigit::transport::negotiate::{
    build_fetch_request, build_push_update, parse_advertisement, parse_report_status, FetchRequest,
    PushCommand,
};
use minigit::transport::pktline::{encode_pkt, read_pkt, Pkt};
use minigit::{Error, Oid};

// ---------------------------------------------------------------------------
// 通用约束
// ---------------------------------------------------------------------------

/// 任何需要真实 git 的用例都必须先调用它：**环境缺失 = panic，不是静默跳过**。
fn require_git() -> String {
    let out = Command::new("git")
        .arg("--version")
        .output()
        .expect("this verification requires a real `git` on PATH (not skipping)");
    assert!(
        out.status.success(),
        "`git --version` failed: real git is required by this verification (not skipping)"
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 客户端调用必须有界：挂死（死循环/死锁）要在 45s 内变成 FAIL，而不是无限等。
fn guarded<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let value = f();
        let _ = tx.send(value);
    });
    match rx.recv_timeout(Duration::from_secs(45)) {
        Ok(value) => {
            let _ = handle.join();
            value
        }
        Err(_) => {
            panic!("{what} did not return within 45s: client hung (infinite loop / deadlock?)")
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> Output {
    require_git();
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git")
}

fn git_ok(dir: &Path, args: &[&str]) -> Output {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {} failed (exit {:?}): {}",
        dir.display(),
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = git_ok(dir, args);
    String::from_utf8(out.stdout)
        .expect("git stdout is UTF-8")
        .trim()
        .to_string()
}

fn write_commit(dir: &Path, name: &str, content: &str, message: &str) {
    std::fs::write(dir.join(name), content.as_bytes()).expect("write fixture file");
    git_ok(dir, &["add", name]);
    git_ok(
        dir,
        &[
            "-c",
            "user.name=V14",
            "-c",
            "user.email=v14@example.invalid",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

/// 一台带 main 分支历史、可用 HTTP 服务的 bare 服务端仓库。
fn bare_server_with_main(root: &Path) -> PathBuf {
    let srv = root.join("srv.git");
    // `init --bare` 默认分支是 master；夹具必须把 HEAD 指到真实存在的 main，
    // 否则 HEAD unborn：广告里没有 HEAD、`git rev-parse HEAD` 会把字面量 "HEAD" 打进 stdout 且 exit 0。
    git_ok(
        root,
        &["init", "-q", "--bare", "-b", "main", srv.to_str().unwrap()],
    );
    let work = root.join("work");
    git_ok(root, &["init", "-q", "-b", "main", work.to_str().unwrap()]);
    write_commit(&work, "a.txt", "one\n", "one");
    git_ok(&work, &["push", "-q", srv.to_str().unwrap(), "main"]);
    srv
}

/// 空的 bare 服务端仓库（receive-pack 用），并打开 HTTP push 服务开关。
fn empty_bare_server(root: &Path) -> PathBuf {
    let srv = root.join("srv.git");
    git_ok(
        root,
        &["init", "-q", "--bare", "-b", "main", srv.to_str().unwrap()],
    );
    git_ok(&srv, &["config", "http.receivepack", "true"]);
    srv
}

// ---------------------------------------------------------------------------
// 脚手架：TcpListener + git http-backend CGI
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Fault {
    /// 真转发 http-backend，用 `Content-Length` 回。
    Normal,
    /// 不跑后端，把 `raw_cgi`（原本来自一次真实 http-backend 调用）原样用
    /// `Transfer-Encoding: chunked` 重放。用于「同一份后端 body、两种 framing」的逐字节对拍
    /// （真实 upload-pack 的 progress 分帧在两次进程间不保证一致，不能拿两次独立调用对拍）。
    ReplayChunked {
        raw_cgi: Arc<Vec<u8>>,
        chunk_size: usize,
        dribble: bool,
    },
    /// 真转发 http-backend，用 `Transfer-Encoding: chunked` 回；
    /// `dribble` 时整段响应逐字节 write+flush（制造分块边界跨 TCP 包）。
    Chunked { chunk_size: usize, dribble: bool },
    /// 不回后端，直接回一个合成状态码 + 可识别 body。
    Status(u16, &'static str),
    /// 200 但既无 `Content-Length` 也无 chunked。
    NoFraming,
    /// 回纯文本 `hello`（根本不是 HTTP）。
    PlainText,
    /// accept 后立即关闭。
    CloseImmediately,
    /// 声称 `Content-Length: 100000` 却只发 7 字节就关。
    TruncatedBody,
}

#[derive(Clone, Debug)]
struct Recorded {
    /// 客户端实际发出的原始 request line + headers。
    head: Vec<u8>,
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// `git http-backend` 的原始 stdout（合成 fault 时为空）。
    cgi_raw: Vec<u8>,
    /// 我们回给客户端的原始 status line + headers。
    response_head: Vec<u8>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn cgi_body(&self) -> &[u8] {
        match self.cgi_raw.windows(4).position(|w| w == b"\r\n\r\n") {
            Some(idx) => &self.cgi_raw[idx + 4..],
            None => &[],
        }
    }
}

struct Scaffold {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
    records: Arc<Mutex<Vec<Recorded>>>,
}

impl Scaffold {
    fn start(root: &Path, fault: Fault) -> Scaffold {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local_addr");
        eprintln!(
            "verify_http_http: scaffold listening on http://{addr} (fault={fault:?}, root={})",
            root.display()
        );
        listener
            .set_nonblocking(true)
            .expect("set_nonblocking on the listener");
        let shutdown = Arc::new(AtomicBool::new(false));
        let records = Arc::new(Mutex::new(Vec::new()));
        let flag = Arc::clone(&shutdown);
        let recs = Arc::clone(&records);
        let root = root.to_path_buf();
        let join = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => handle_connection(stream, &root, fault.clone(), &recs),
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Scaffold {
            addr,
            shutdown,
            join: Some(join),
            records,
        }
    }

    fn url(&self, repo: &str) -> String {
        format!("http://{}/{repo}", self.addr)
    }

    /// 客户端返回时服务端线程可能还没 push 记录：轮询等待，最多 5s（不静默跳过）。
    fn record(&self, index: usize) -> Recorded {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(record) = self
                .records
                .lock()
                .expect("records mutex")
                .get(index)
                .cloned()
            {
                return record;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "scaffold recorded fewer than {} requests within 5s",
                index + 1
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Scaffold {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    root: &Path,
    fault: Fault,
    records: &Mutex<Vec<Recorded>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));

    if let Fault::CloseImmediately = fault {
        drop(stream);
        return;
    }

    let head = match read_head(&mut stream) {
        Ok(head) if !head.is_empty() => head,
        _ => return,
    };
    let (method, target, headers) = parse_head(&head);
    let length = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    if length > 0 && stream.read_exact(&mut body).is_err() {
        return;
    }

    let mut record = Recorded {
        head,
        method: method.clone(),
        target: target.clone(),
        headers,
        body,
        cgi_raw: Vec::new(),
        response_head: Vec::new(),
    };

    match fault {
        Fault::PlainText => {
            let _ = stream.write_all(b"hello");
            let _ = stream.flush();
        }
        Fault::Status(code, reason) => {
            let text = format!("scaffold fault body for HTTP {code}");
            let head = format!(
                "HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                text.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(text.as_bytes());
            let _ = stream.flush();
        }
        Fault::NoFraming => {
            let text = b"body without any framing";
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n");
            let _ = stream.write_all(text);
            let _ = stream.flush();
        }
        Fault::TruncatedBody => {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\nabcdefg");
            let _ = stream.flush();
        }
        Fault::Normal | Fault::Chunked { .. } => {
            let backend = run_backend(
                root,
                &record.method,
                &record.target,
                &record.headers,
                &record.body,
            );
            let (status, reason, cgi_headers, cgi_body) = parse_cgi_output(&backend.stdout);
            record.cgi_raw = backend.stdout.clone();
            let framing = match fault {
                Fault::Chunked {
                    chunk_size,
                    dribble,
                } => FrameMode::Chunked {
                    chunk_size,
                    dribble,
                },
                _ => FrameMode::ContentLength,
            };
            let response_head = write_response(
                &mut stream,
                status,
                &reason,
                &cgi_headers,
                cgi_body,
                framing,
            );
            record.response_head = response_head;
        }
        Fault::ReplayChunked {
            raw_cgi,
            chunk_size,
            dribble,
        } => {
            let (status, reason, cgi_headers, cgi_body) = parse_cgi_output(&raw_cgi);
            record.cgi_raw = raw_cgi.as_ref().clone();
            record.response_head = write_response(
                &mut stream,
                status,
                &reason,
                &cgi_headers,
                cgi_body,
                FrameMode::Chunked {
                    chunk_size,
                    dribble,
                },
            );
        }
        Fault::CloseImmediately => unreachable!("handled above"),
    }

    records.lock().expect("records mutex").push(record);
}

fn read_head(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = stream.read(&mut byte)?;
        if read == 0 {
            return Ok(head);
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") || head.len() > 65536 {
            return Ok(head);
        }
    }
}

fn parse_head(head: &[u8]) -> (String, String, Vec<(String, String)>) {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    (method, target, headers)
}

fn parse_cgi_output(raw: &[u8]) -> (u16, String, Vec<String>, &[u8]) {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("git http-backend output has a header/body separator");
    let header_text = String::from_utf8_lossy(&raw[..split]).into_owned();
    let body = &raw[split + 4..];
    let mut status = 200u16;
    let mut reason = "OK".to_string();
    let mut headers = Vec::new();
    for line in header_text.split("\r\n") {
        if line.is_empty() {
            continue;
        }
        match line.split_once(':') {
            Some((name, value)) if name.eq_ignore_ascii_case("status") => {
                let value = value.trim();
                let mut parts = value.splitn(2, ' ');
                status = parts.next().unwrap_or("200").parse().unwrap_or(200);
                reason = parts.next().unwrap_or("OK").to_string();
            }
            _ => headers.push(line.to_string()),
        }
    }
    (status, reason, headers, body)
}

#[derive(Clone, Copy)]
enum FrameMode {
    ContentLength,
    Chunked { chunk_size: usize, dribble: bool },
}

/// 像真实 web server 一样回给客户端：CGI 的 `Status:` 伪头变成状态行，
/// 其余头原样保留，再自己补 framing（真实 `git http-backend` 自己不带 framing）。
fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    cgi_headers: &[String],
    body: &[u8],
    mode: FrameMode,
) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for header in cgi_headers {
        head.push_str(header);
        head.push_str("\r\n");
    }
    let mut framed = Vec::new();
    match mode {
        FrameMode::ContentLength => {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
            head.push_str("Connection: close\r\n\r\n");
            framed.extend_from_slice(&head.into_bytes());
            framed.extend_from_slice(body);
        }
        FrameMode::Chunked {
            chunk_size,
            dribble: _,
        } => {
            head.push_str("Transfer-Encoding: chunked\r\n");
            head.push_str("Connection: close\r\n\r\n");
            let mut full = head.into_bytes();
            let chunk_size = chunk_size.max(1);
            if body.is_empty() {
                full.extend_from_slice(b"0\r\n\r\n");
            } else {
                for chunk in body.chunks(chunk_size) {
                    full.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                    full.extend_from_slice(chunk);
                    full.extend_from_slice(b"\r\n");
                }
                full.extend_from_slice(b"0\r\n\r\n");
            }
            framed = full;
        }
    }
    if let FrameMode::Chunked { dribble: true, .. } = mode {
        for byte in &framed {
            let _ = stream.write_all(&[*byte]);
            let _ = stream.flush();
            thread::sleep(Duration::from_millis(1));
        }
    } else {
        let _ = stream.write_all(&framed);
        let _ = stream.flush();
    }
    framed
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|idx| framed[..idx + 4].to_vec())
        .unwrap_or_else(|| framed.clone())
}

/// 真起 `git http-backend`（CGI），环境变量就地 `.env()` 传，不 export 到 shell。
fn run_backend(
    root: &Path,
    method: &str,
    target: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Output {
    let (path_info, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (target.to_string(), String::new()),
    };
    let content_type = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let git_protocol = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("git-protocol"))
        .map(|(_, v)| v.clone());

    let mut cmd = Command::new("git");
    cmd.arg("http-backend")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", &path_info)
        .env("QUERY_STRING", &query)
        .env("REQUEST_METHOD", method)
        .env("CONTENT_TYPE", &content_type)
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("SERVER_PROTOCOL", "HTTP/1.1");
    if method == "POST" {
        cmd.env("CONTENT_LENGTH", body.len().to_string());
    }
    if let Some(protocol) = git_protocol {
        cmd.env("GIT_PROTOCOL", protocol);
    }

    let mut child = cmd.spawn().expect("spawn git http-backend");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(body)
        .expect("write body to git http-backend");
    child.wait_with_output().expect("wait for git http-backend")
}

// ---------------------------------------------------------------------------
// (B1) 请求形状
// ---------------------------------------------------------------------------

#[test]
fn request_shape_matches_smart_http_contract() {
    let version = require_git();
    eprintln!("verify_http_http: real git = {version}");
    let tmp = tempfile::tempdir().expect("tempdir");
    let srv = bare_server_with_main(tmp.path());
    let scaffold = Scaffold::start(tmp.path(), Fault::Normal);
    let info_url = scaffold.url("srv.git");

    let info = guarded("info_refs", {
        let url = info_url.clone();
        move || HttpRemote::new(&url).info_refs("git-upload-pack")
    })
    .expect("info_refs against a real http-backend must succeed");
    assert!(!info.is_empty());

    let recorded = scaffold.record(0);
    assert_eq!(recorded.method, "GET");
    assert_eq!(
        recorded.target, "/srv.git/info/refs?service=git-upload-pack",
        "raw request target"
    );
    assert!(
        recorded
            .head
            .starts_with(b"GET /srv.git/info/refs?service=git-upload-pack HTTP/1.1\r\n"),
        "raw request line was {:?}",
        String::from_utf8_lossy(&recorded.head)
    );
    assert_eq!(
        recorded.header("Host"),
        Some(scaffold.addr.to_string().as_str()),
        "Host header must carry the real port"
    );
    let ua = recorded.header("User-Agent").expect("User-Agent header");
    assert!(
        ua.starts_with("git/") && ua[4..].starts_with(|c: char| c.is_ascii_digit()),
        "User-Agent must look like a git client, got {ua:?}"
    );
    // W4/C-27：`info_refs` 的 Accept 曾多一层 `git-`（T14 缺陷，V14 发现、T14b 修复）；
    // 现在与真实 git 2.55.0 逐字节一致 = `*/*`。
    let accept = recorded
        .header("Accept")
        .expect("Accept header must be present");
    assert_eq!(
        accept, "*/*",
        "GET /info/refs must send the same Accept as real git, got {accept:?}"
    );
    assert!(
        !recorded
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("git-protocol")),
        "client must not send Git-Protocol (we only claim v0 support)"
    );
    assert!(
        recorded.body.is_empty(),
        "GET /info/refs must have an empty body, got {} bytes",
        recorded.body.len()
    );
    // v0 自证：后端原始输出的第一个 pkt 是 service 行，而不是 `version 2`。
    let cgi_body = recorded.cgi_body();
    assert!(
        cgi_body.starts_with(&encode_pkt(b"# service=git-upload-pack\n")),
        "http-backend must have answered with a v0 service line, got {:?}",
        String::from_utf8_lossy(&cgi_body[..cgi_body.len().min(80)])
    );
    assert!(
        !contains(cgi_body, b"version 2"),
        "advertisement must be protocol v0, not v2"
    );

    // POST /git-upload-pack：Content-Type / Content-Length / body 逐字节。
    let payload = build_fetch_request(
        &FetchRequest {
            wants: vec![Oid::zeros()],
            haves: vec![],
            done: true,
        },
        &["side-band-64k"],
    );
    let remote_url = scaffold.url("srv.git");
    let payload_for_call = payload.clone();
    let response = guarded("upload_pack(shape)", move || {
        HttpRemote::new(&remote_url).upload_pack(&payload_for_call)
    });
    assert!(
        response.is_ok(),
        "synthetic body exercises the request path"
    );
    let recorded = scaffold.record(1);
    assert_eq!(recorded.method, "POST");
    assert_eq!(recorded.target, "/srv.git/git-upload-pack");
    assert_eq!(
        recorded.header("Content-Type"),
        Some("application/x-git-upload-pack-request")
    );
    assert_eq!(
        recorded.header("Accept"),
        Some("application/x-git-upload-pack-result")
    );
    assert_eq!(
        recorded.header("Content-Length"),
        Some(payload.len().to_string().as_str()),
        "Content-Length must match the body we send"
    );
    assert_eq!(
        recorded.body, payload,
        "raw POST body must be byte-identical"
    );

    // 顺带校正「User-Agent 决定协议版本」：真实 git 2.55.0 只认 Git-Protocol。
    let _ = srv;
}

#[test]
fn info_refs_advertisement_matches_real_git_oracles() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let srv = bare_server_with_main(tmp.path());
    let scaffold = Scaffold::start(tmp.path(), Fault::Normal);
    let remote = HttpRemote::new(&scaffold.url("srv.git"));

    let bytes = remote
        .info_refs("git-upload-pack")
        .expect("info_refs must succeed against a real git http-backend");

    // 独立断言 1：服务行 + flush 被逐字节剥掉，其余原样。
    let mut expected_prefix = encode_pkt(b"# service=git-upload-pack\n");
    expected_prefix.extend_from_slice(b"0000");
    let cgi_body = scaffold.record(0).cgi_body().to_vec();
    assert!(
        cgi_body.starts_with(&expected_prefix),
        "oracle fixture must produce the service line + flush prefix"
    );
    assert_eq!(
        bytes,
        cgi_body[expected_prefix.len()..].to_vec(),
        "info_refs must return exactly the advertisement after `# service=` + the first flush-pkt"
    );

    let advertisement = parse_advertisement(&bytes).expect("advertisement must parse");

    // 独立 oracle A：for-each-ref / rev-parse / symbolic-ref。
    let mut oracle = BTreeMap::new();
    let listing = git_stdout(&srv, &["for-each-ref", "--format=%(objectname) %(refname)"]);
    for line in listing.lines() {
        let (oid, name) = line.split_once(' ').expect("for-each-ref line");
        oracle.insert(name.to_string(), oid.to_string());
    }
    oracle.insert("HEAD".to_string(), git_stdout(&srv, &["rev-parse", "HEAD"]));
    let actual: BTreeMap<String, String> = advertisement
        .refs
        .iter()
        .map(|(name, oid)| (name.clone(), oid.to_hex()))
        .collect();
    assert_eq!(
        actual, oracle,
        "advertised refs must equal `git for-each-ref` + HEAD"
    );

    // 独立 oracle B：`git ls-remote`（明文，不是 pkt-line）。
    let ls_remote = git_stdout(&srv, &["ls-remote", "."]);
    assert!(
        !ls_remote.starts_with("00"),
        "sanity: `git ls-remote` stdout is not pkt-line ({ls_remote:?})"
    );
    let mut ls_remote_refs = BTreeMap::new();
    for line in ls_remote.lines() {
        let (oid, name) = line.split_once('\t').expect("ls-remote line");
        ls_remote_refs.insert(name.to_string(), oid.to_string());
    }
    assert_eq!(
        actual, ls_remote_refs,
        "advertised refs must equal `git ls-remote`"
    );

    // symref：HEAD -> refs/heads/main，与真实 git 一致。
    assert_eq!(
        advertisement.head_symref(),
        Some("refs/heads/main"),
        "HEAD symref must match the server repository"
    );
    assert_eq!(
        git_stdout(&srv, &["symbolic-ref", "HEAD"]),
        "refs/heads/main"
    );
}

// ---------------------------------------------------------------------------
// (B1b) 与真实 git 客户端的请求形状对拍（真值 = git 自己发出的字节）
// ---------------------------------------------------------------------------

fn header_of(record: &Recorded, name: &str) -> Option<String> {
    record.header(name).map(|value| value.to_string())
}

#[test]
fn request_headers_match_the_real_git_client() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    bare_server_with_main(tmp.path());
    let scaffold = Scaffold::start(tmp.path(), Fault::Normal);
    let url = scaffold.url("srv.git");

    // 真值来源 1：真实 git 客户端（强制 v0）自己发出的 GET /info/refs。
    let ls_remote = Command::new("git")
        .args(["-c", "protocol.version=0", "ls-remote"])
        .arg(&url)
        .output()
        .expect("run git ls-remote");
    assert!(
        ls_remote.status.success(),
        "git ls-remote: {}",
        String::from_utf8_lossy(&ls_remote.stderr)
    );
    let git_get = scaffold.record(0);

    HttpRemote::new(&url)
        .info_refs("git-upload-pack")
        .expect("info_refs");
    let our_get = scaffold.record(1);

    assert_eq!(our_get.method, git_get.method, "method must match real git");
    assert_eq!(our_get.target, git_get.target, "target must match real git");
    assert_eq!(
        header_of(&our_get, "Host"),
        header_of(&git_get, "Host"),
        "Host must match real git"
    );
    assert_eq!(
        header_of(&our_get, "User-Agent"),
        header_of(&git_get, "User-Agent"),
        "User-Agent must match real git"
    );
    assert_eq!(
        header_of(&git_get, "Git-Protocol"),
        None,
        "sanity: `-c protocol.version=0` real git does not ask for v2"
    );
    assert_eq!(
        header_of(&our_get, "Git-Protocol"),
        None,
        "HttpRemote must not ask for v2"
    );
    assert!(
        our_get.body.is_empty(),
        "GET /info/refs must have no body, like real git"
    );

    // 真值来源 2：真实 git 客户端 fetch 时的 POST /git-upload-pack。
    let client = tmp.path().join("fetchcli");
    git_ok(
        tmp.path(),
        &["init", "-q", "-b", "main", client.to_str().unwrap()],
    );
    let fetch = Command::new("git")
        .arg("-C")
        .arg(&client)
        .args(["-c", "protocol.version=0", "fetch", "-q", &url, "HEAD"])
        .output()
        .expect("run git fetch");
    assert!(
        fetch.status.success(),
        "git fetch: {}",
        String::from_utf8_lossy(&fetch.stderr)
    );
    let git_post = scaffold.record(3);

    let advertisement = parse_advertisement(
        &HttpRemote::new(&url)
            .info_refs("git-upload-pack")
            .expect("second info_refs"),
    )
    .expect("advertisement");
    let want = advertisement.get("HEAD").expect("fixture advertises HEAD");
    let request = build_fetch_request(
        &FetchRequest {
            wants: vec![want],
            haves: vec![],
            done: true,
        },
        &["side-band-64k"],
    );
    let _ = HttpRemote::new(&url).upload_pack(&request);
    let our_post = scaffold.record(5);

    assert_eq!(our_post.method, git_post.method);
    assert_eq!(our_post.target, git_post.target);
    assert_eq!(
        header_of(&our_post, "Content-Type"),
        header_of(&git_post, "Content-Type"),
        "POST Content-Type must match real git"
    );
    assert_eq!(
        header_of(&our_post, "Accept"),
        header_of(&git_post, "Accept"),
        "POST Accept must match real git"
    );
    assert_eq!(
        header_of(&our_post, "Content-Length"),
        Some(our_post.body.len().to_string()),
        "our Content-Length must equal our body length"
    );
    assert!(
        !our_post.body.is_empty(),
        "sanity: the fetch request body we sent is non-empty"
    );
}

/// `info_refs` 的 GET `Accept` 必须与真实 git 逐字节一致（`*/*`）。
///
/// 历史：V14 曾用本用例**钉住**一个低严重度缺陷（`application/x-git-git-upload-pack-advertisement`
/// 多一层 `git-`）；T14b 修复后由 controller（W4/C-27）把它转正成常驻回归 —— 三条断言的语义
/// 从「我们与 git 不同」翻转为「我们与 git 相同」。
#[test]
fn info_refs_accept_header_matches_real_git() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    bare_server_with_main(tmp.path());
    let scaffold = Scaffold::start(tmp.path(), Fault::Normal);
    let url = scaffold.url("srv.git");

    let ls_remote = Command::new("git")
        .args(["-c", "protocol.version=0", "ls-remote"])
        .arg(&url)
        .output()
        .expect("run git ls-remote");
    assert!(ls_remote.status.success());
    let real_accept = header_of(&scaffold.record(0), "Accept").expect("real git sends Accept");

    HttpRemote::new(&url)
        .info_refs("git-upload-pack")
        .expect("info_refs");
    let our_accept = header_of(&scaffold.record(1), "Accept").expect("HttpRemote sends Accept");

    eprintln!("HttpRemote Accept={our_accept:?}, real git 2.55.0 Accept={real_accept:?}");
    assert_eq!(
        our_accept, real_accept,
        "HttpRemote must send exactly the same Accept as real git"
    );
    assert_eq!(
        our_accept, "*/*",
        "real git sends Accept: */* for GET /info/refs"
    );
}

// ---------------------------------------------------------------------------
// (B3) 真实上传链路
// ---------------------------------------------------------------------------

#[test]
fn upload_pack_response_unpacks_with_real_git() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let srv = bare_server_with_main(tmp.path());
    let scaffold = Scaffold::start(tmp.path(), Fault::Normal);
    let remote = HttpRemote::new(&scaffold.url("srv.git"));

    let advertisement = parse_advertisement(
        &remote
            .info_refs("git-upload-pack")
            .expect("info_refs must succeed"),
    )
    .expect("advertisement must parse");
    let want = advertisement
        .get("HEAD")
        .or_else(|| advertisement.get("refs/heads/main"))
        .expect("fixture advertises HEAD");
    assert_eq!(
        want.to_hex(),
        git_stdout(&srv, &["rev-parse", "HEAD"]),
        "the want oid must be the server's real HEAD"
    );

    let request = build_fetch_request(
        &FetchRequest {
            wants: vec![want],
            haves: vec![],
            done: true,
        },
        &["side-band-64k"],
    );
    let response = remote
        .upload_pack(&request)
        .expect("upload_pack must succeed against a real git http-backend");

    // 逐帧解析：ack + side-band 通道 1 的 pack。
    let mut cursor = std::io::Cursor::new(response.as_slice());
    let mut pack = Vec::new();
    let mut progress = Vec::new();
    let mut saw_ack = false;
    loop {
        match read_pkt(&mut cursor).expect("upload-pack response must be valid pkt-line") {
            Pkt::Data(data) => {
                if data == b"NAK\n" || data.starts_with(b"ACK ") {
                    saw_ack = true;
                    continue;
                }
                assert!(!data.is_empty(), "side-band frame without a channel byte");
                match data[0] {
                    1 => pack.extend_from_slice(&data[1..]),
                    2 => progress.extend_from_slice(&data[1..]),
                    3 => panic!(
                        "server sent a fatal side-band error: {:?}",
                        String::from_utf8_lossy(&data[1..])
                    ),
                    other => panic!("unexpected side-band channel {other}"),
                }
            }
            Pkt::Flush => break,
            other => panic!("unexpected frame in upload-pack response: {other:?}"),
        }
    }
    assert!(saw_ack, "protocol v0 must start with NAK/ACK");
    assert!(
        pack.starts_with(b"PACK"),
        "channel 1 must carry a real packfile (got {:?})",
        String::from_utf8_lossy(&pack[..pack.len().min(16)])
    );

    let pack_file = tmp.path().join("payload.pack");
    std::fs::write(&pack_file, &pack).expect("write pack");
    let client = tmp.path().join("client.git");
    git_ok(
        tmp.path(),
        &["init", "-q", "--bare", client.to_str().unwrap()],
    );
    let unpack = Command::new("git")
        .arg("-C")
        .arg(&client)
        .args(["unpack-objects", "-q"])
        .stdin(File::open(&pack_file).expect("open pack"))
        .output()
        .expect("run git unpack-objects");
    assert!(
        unpack.status.success(),
        "git unpack-objects must accept the side-band pack: {}",
        String::from_utf8_lossy(&unpack.stderr)
    );
    assert_eq!(
        git_stdout(&client, &["cat-file", "-t", &want.to_hex()]),
        "commit",
        "the fetched object must be materialized by real git"
    );
    let _ = srv;
}

#[test]
fn receive_pack_round_trips_report_status_against_real_git() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let srv = empty_bare_server(tmp.path());
    let client = tmp.path().join("cli");
    git_ok(
        tmp.path(),
        &["init", "-q", "-b", "main", client.to_str().unwrap()],
    );
    write_commit(&client, "f.txt", "pushed\n", "pushed");
    let new = git_stdout(&client, &["rev-parse", "HEAD"]);
    let pack = git_ok(&client, &["pack-objects", "--stdout", "--all"]).stdout;
    assert!(
        pack.starts_with(b"PACK"),
        "pack-objects oracle must emit a pack"
    );

    let mut body = build_push_update(
        &[PushCommand {
            old: Oid::zeros(),
            new: Oid::from_hex(&new).expect("valid oid"),
            name: "refs/heads/main".to_string(),
        }],
        &["report-status"],
    )
    .expect("build_push_update");
    body.extend_from_slice(&pack);

    let scaffold = Scaffold::start(tmp.path(), Fault::Normal);
    let remote = HttpRemote::new(&scaffold.url("srv.git"));
    let response = remote
        .receive_pack(&body)
        .expect("receive_pack must succeed when http.receivepack=true");
    let recorded = scaffold.record(0);
    assert_eq!(recorded.target, "/srv.git/git-receive-pack");
    assert_eq!(recorded.method, "POST");

    let statuses = parse_report_status(&response).expect("real report-status must parse");
    let lookup = |name: &str| {
        statuses
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, status)| status.clone())
    };
    assert_eq!(
        lookup("unpack").as_deref(),
        Some("ok"),
        "statuses={statuses:?}"
    );
    assert_eq!(
        lookup("refs/heads/main").as_deref(),
        Some("ok"),
        "statuses={statuses:?}"
    );
    assert_eq!(
        git_stdout(&srv, &["rev-parse", "refs/heads/main"]),
        new,
        "the server ref must actually move"
    );

    // push 侧的 GET /info/refs?service=git-receive-pack 分支也走一遍真实 http-backend。
    let push_advertisement = parse_advertisement(
        &remote
            .info_refs("git-receive-pack")
            .expect("info_refs(git-receive-pack) must succeed"),
    )
    .expect("receive-pack advertisement must parse");
    assert_eq!(
        push_advertisement
            .get("refs/heads/main")
            .map(|oid| oid.to_hex()),
        Some(new.clone()),
        "the receive-pack advertisement must show the just-pushed oid"
    );
}

// ---------------------------------------------------------------------------
// (B4) chunked 分支
// ---------------------------------------------------------------------------

#[test]
fn chunked_response_matches_content_length_byte_for_byte() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    bare_server_with_main(tmp.path());

    let reference = Scaffold::start(tmp.path(), Fault::Normal);
    // chunk_size 必须取 hex 里含字母的值（11 -> "b"），否则 hex/dec 解析无法区分。
    let chunked = Scaffold::start(
        tmp.path(),
        Fault::Chunked {
            chunk_size: 11,
            dribble: false,
        },
    );

    let from_length = HttpRemote::new(&reference.url("srv.git"))
        .info_refs("git-upload-pack")
        .expect("Content-Length branch");
    let from_chunked = HttpRemote::new(&chunked.url("srv.git"))
        .info_refs("git-upload-pack")
        .expect("chunked branch");
    assert_eq!(
        from_length, from_chunked,
        "chunked decoding must produce exactly the same bytes"
    );
    assert!(
        chunked
            .record(0)
            .response_head
            .windows(28)
            .any(|w| w.eq_ignore_ascii_case(b"Transfer-Encoding: chunked\r\n")),
        "the fixture really used chunked framing"
    );

    // POST /git-upload-pack：真实 http-backend 的 progress 分帧在两次进程间不保证一致，
    // 所以用**同一份后端 body** 再做一次 chunked framing 重放，然后逐字节对拍解码结果。
    let advertisement = parse_advertisement(&from_length).expect("advertisement");
    let want = advertisement.get("HEAD").expect("HEAD");
    let request = build_fetch_request(
        &FetchRequest {
            wants: vec![want],
            haves: vec![],
            done: true,
        },
        &["side-band-64k"],
    );
    let post_length = HttpRemote::new(&reference.url("srv.git"))
        .upload_pack(&request)
        .expect("Content-Length POST");
    let replay_raw = reference.record(1).cgi_raw.clone();
    assert!(
        contains(&replay_raw, b"PACK"),
        "sanity: the replayed backend body carries a pack"
    );
    let replay = Scaffold::start(
        tmp.path(),
        Fault::ReplayChunked {
            raw_cgi: Arc::new(replay_raw),
            chunk_size: 13,
            dribble: false,
        },
    );
    let post_chunked = HttpRemote::new(&replay.url("srv.git"))
        .upload_pack(&request)
        .expect("chunked POST");
    assert_eq!(
        post_length, post_chunked,
        "the same backend body must decode identically under both framings"
    );
    assert!(
        post_length.len() > 100,
        "sanity: the pack stream is multi-chunk"
    );
    assert!(
        contains(
            &replay.record(0).response_head,
            b"Transfer-Encoding: chunked"
        ),
        "the replay fixture really used chunked framing"
    );
}

#[test]
fn chunked_across_tcp_packet_boundaries_matches() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    bare_server_with_main(tmp.path());

    let reference = Scaffold::start(tmp.path(), Fault::Normal);
    let dribble = Scaffold::start(
        tmp.path(),
        Fault::Chunked {
            chunk_size: 11,
            dribble: true,
        },
    );

    let expected = HttpRemote::new(&reference.url("srv.git"))
        .info_refs("git-upload-pack")
        .expect("Content-Length branch");
    let observed = guarded("dribbled chunked info_refs", {
        let url = dribble.url("srv.git");
        move || HttpRemote::new(&url).info_refs("git-upload-pack")
    })
    .expect("1-byte-at-a-time chunked response must decode");
    assert_eq!(
        expected, observed,
        "chunk boundaries split across TCP writes must not change the decoded body"
    );

    // POST 也做一次：同一份后端 body，用逐字节 dribble 的 chunked framing 重放。
    let advertisement = parse_advertisement(&expected).expect("advertisement");
    let want = advertisement.get("HEAD").expect("HEAD");
    let request = build_fetch_request(
        &FetchRequest {
            wants: vec![want],
            haves: vec![],
            done: true,
        },
        &["side-band-64k"],
    );
    let expected_post = HttpRemote::new(&reference.url("srv.git"))
        .upload_pack(&request)
        .expect("Content-Length POST");
    let replay = Scaffold::start(
        tmp.path(),
        Fault::ReplayChunked {
            raw_cgi: Arc::new(reference.record(1).cgi_raw.clone()),
            chunk_size: 27,
            dribble: true,
        },
    );
    let observed_post = guarded("dribbled chunked upload_pack", {
        let url = replay.url("srv.git");
        let request = request.clone();
        move || HttpRemote::new(&url).upload_pack(&request)
    })
    .expect("dribbled chunked POST must decode");
    assert_eq!(
        expected_post, observed_post,
        "1-byte-at-a-time chunked framing must decode to the same bytes"
    );
}

// ---------------------------------------------------------------------------
// (B5) 反例：清晰 Err、不 panic、不死循环
// ---------------------------------------------------------------------------

/// `haystack` 里是否包含 `needle`（避免手算 windows 长度写错）。
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn assert_clear_error(what: &str, result: minigit::Result<Vec<u8>>) -> String {
    match result {
        Ok(bytes) => panic!(
            "{what}: expected a clear Err, got Ok({} bytes)",
            bytes.len()
        ),
        Err(err) => {
            let message = err.to_string();
            assert!(
                !message.is_empty(),
                "{what}: error message must not be empty"
            );
            eprintln!("verify_http_http: {what} -> {message}");
            message
        }
    }
}

#[test]
fn http_error_status_surfaces_code_and_body() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    for (code, reason) in [(404u16, "Not Found"), (500, "Internal Server Error")] {
        let scaffold = Scaffold::start(tmp.path(), Fault::Status(code, reason));
        let url = scaffold.url("srv.git");
        let message = guarded("status fault", move || {
            assert_clear_error(
                "status fault",
                HttpRemote::new(&url).info_refs("git-upload-pack"),
            )
        });
        assert!(
            message.contains(&code.to_string()),
            "error must mention the HTTP status {code}: {message}"
        );
        assert!(
            message.contains("scaffold fault body"),
            "error must include a body snippet: {message}"
        );
    }
}

#[test]
fn missing_framing_is_a_clear_error() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let scaffold = Scaffold::start(tmp.path(), Fault::NoFraming);
    let url = scaffold.url("srv.git");
    let message = guarded("no framing", move || {
        assert_clear_error(
            "200 without Content-Length/chunked",
            HttpRemote::new(&url).info_refs("git-upload-pack"),
        )
    });
    assert!(
        message.contains("Content-Length"),
        "the error should name the missing framing: {message}"
    );
}

#[test]
fn plain_text_response_is_a_clear_error() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let scaffold = Scaffold::start(tmp.path(), Fault::PlainText);
    let url = scaffold.url("srv.git");
    guarded("plain text", move || {
        assert_clear_error(
            "non-HTTP `hello`",
            HttpRemote::new(&url).info_refs("git-upload-pack"),
        )
    });
}

#[test]
fn premature_close_is_a_clear_error() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let scaffold = Scaffold::start(tmp.path(), Fault::CloseImmediately);
    let url = scaffold.url("srv.git");
    guarded("premature close", move || {
        assert_clear_error(
            "connection closed by the peer",
            HttpRemote::new(&url).info_refs("git-upload-pack"),
        )
    });
}

#[test]
fn truncated_body_is_a_clear_error() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let scaffold = Scaffold::start(tmp.path(), Fault::TruncatedBody);
    let url = scaffold.url("srv.git");
    guarded("truncated body", move || {
        assert_clear_error(
            "Content-Length promises more than the peer sends",
            HttpRemote::new(&url).info_refs("git-upload-pack"),
        )
    });
}

#[test]
fn https_url_is_unsupported() {
    require_git();
    let err = HttpRemote::new("https://example.invalid/srv.git")
        .info_refs("git-upload-pack")
        .expect_err("https:// must not be attempted");
    assert!(
        matches!(err, Error::Unsupported(_)),
        "https:// must map to Error::Unsupported, got {err:?}"
    );
    let err = HttpRemote::new("https://example.invalid/srv.git")
        .upload_pack(b"0000")
        .expect_err("https:// upload_pack must not be attempted");
    assert!(matches!(err, Error::Unsupported(_)), "got {err:?}");
    let err = HttpRemote::new("https://example.invalid/srv.git")
        .receive_pack(b"0000")
        .expect_err("https:// receive_pack must not be attempted");
    assert!(matches!(err, Error::Unsupported(_)), "got {err:?}");
}

// ---------------------------------------------------------------------------
// 真值前提复核（对任务书/作者自述的独立判定）
// ---------------------------------------------------------------------------

fn backend_body(root: &Path, path_info: &str, query: &str, env_extra: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new("git");
    cmd.arg("http-backend")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path_info)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", "GET")
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("SERVER_PROTOCOL", "HTTP/1.1");
    for (key, value) in env_extra {
        cmd.env(key, value);
    }
    let mut child = cmd.spawn().expect("spawn git http-backend");
    drop(child.stdin.take());
    child.wait_with_output().expect("wait for git http-backend")
}

#[test]
fn premise_git_protocol_header_not_user_agent_selects_version() {
    let version = require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    bare_server_with_main(tmp.path());

    let v0 = backend_body(
        tmp.path(),
        "/srv.git/info/refs",
        "service=git-upload-pack",
        &[("HTTP_USER_AGENT", "git/2.55.0")],
    );
    let v0_body = &v0.stdout;
    assert!(
        contains(v0_body, b"# service=git-upload-pack"),
        "without GIT_PROTOCOL the backend must emit the v0 service line"
    );
    assert!(!contains(v0_body, b"version 2"));

    let v2 = backend_body(
        tmp.path(),
        "/srv.git/info/refs",
        "service=git-upload-pack",
        &[
            ("GIT_PROTOCOL", "version=2"),
            ("HTTP_USER_AGENT", "definitely-not-a-git-client/1.0"),
        ],
    );
    assert!(
        contains(&v2.stdout, b"version 2\n"),
        "GIT_PROTOCOL=version=2 must switch the backend to v2 even with a bogus User-Agent"
    );
    eprintln!(
        "verify_http_http: {version}: GIT_PROTOCOL (not User-Agent) selects the protocol version"
    );
}

#[test]
fn premise_http_backend_emits_no_framing() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    bare_server_with_main(tmp.path());
    let out = backend_body(
        tmp.path(),
        "/srv.git/info/refs",
        "service=git-upload-pack",
        &[],
    );
    let split = out
        .stdout
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("CGI header separator");
    let headers = String::from_utf8_lossy(&out.stdout[..split]).to_ascii_lowercase();
    assert!(
        !headers.contains("content-length"),
        "real git http-backend must not emit Content-Length: {headers}"
    );
    assert!(
        !headers.contains("transfer-encoding"),
        "real git http-backend must not emit Transfer-Encoding: {headers}"
    );
}

#[test]
fn premise_http_backend_requires_dechunked_request_body() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let srv = bare_server_with_main(tmp.path());
    let oid = git_stdout(&srv, &["rev-parse", "HEAD"]);
    let mut request = encode_pkt(format!("want {oid} side-band-64k thin-pack\n").as_bytes());
    request.extend_from_slice(b"0000");
    request.extend_from_slice(&encode_pkt(b"done\n"));

    // 真 web server 会在转发前 de-chunk；这里把 chunked 原始字节直接喂给 CGI。
    let mut chunked_body = Vec::new();
    for chunk in request.chunks(16) {
        chunked_body.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        chunked_body.extend_from_slice(chunk);
        chunked_body.extend_from_slice(b"\r\n");
    }
    chunked_body.extend_from_slice(b"0\r\n\r\n");

    let out = Command::new("git")
        .arg("http-backend")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_PROJECT_ROOT", tmp.path())
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", "/srv.git/git-upload-pack")
        .env("QUERY_STRING", "")
        .env("REQUEST_METHOD", "POST")
        .env("CONTENT_TYPE", "application/x-git-upload-pack-request")
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("TRANSFER_ENCODING", "chunked")
        .env("LC_ALL", "C") // W4/C-27: 该用例比对 git 的英文 stderr，必须与 locale 解耦
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(&chunked_body)?;
            child.wait_with_output()
        })
        .expect("run git http-backend with a chunked body");

    assert!(
        !out.status.success(),
        "the CGI itself must not silently accept a chunked request body (status {:?})",
        out.status.code()
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("protocol error"),
        "expected a git protocol error, got stderr {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn premise_receive_pack_needs_http_receivepack_and_unchecked_out_target() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    // (1) bare 仓库但未开 http.receivepack -> CGI 回 Status: 403。
    let srv = bare_server_with_main(tmp.path());
    let disabled = backend_post(tmp.path(), "/srv.git/git-receive-pack");
    assert!(
        String::from_utf8_lossy(&disabled.stdout).contains("403"),
        "without http.receivepack the backend must answer 403: {:?}",
        String::from_utf8_lossy(&disabled.stdout)
    );
    // (2) 打开开关后不再 403。
    git_ok(&srv, &["config", "http.receivepack", "true"]);
    let enabled = backend_post(tmp.path(), "/srv.git/git-receive-pack");
    assert!(
        !String::from_utf8_lossy(&enabled.stdout).contains("403"),
        "with http.receivepack=true the backend must not answer 403"
    );

    // (3) 「必须 bare」是过度概括：非 bare 仓库里，更新的目标分支只要未被检出就能推。
    let work = tmp.path().join("work");
    std::fs::write(work.join("a.txt"), b"one\ntwo changed\n").expect("write");
    git_ok(&work, &["add", "a.txt"]);
    git_ok(
        &work,
        &[
            "-c",
            "user.name=V14",
            "-c",
            "user.email=v14@example.invalid",
            "commit",
            "-q",
            "-m",
            "two",
        ],
    );
    let ahead = tmp.path().join("ahead");
    let ahead_path = ahead.to_str().unwrap().to_string();
    git_ok(
        tmp.path(),
        &["clone", "-q", work.to_str().unwrap(), &ahead_path],
    );
    std::fs::write(ahead.join("a.txt"), b"one\ntwo\nthree\n").expect("write");
    git_ok(&ahead, &["add", "a.txt"]);
    git_ok(
        &ahead,
        &[
            "-c",
            "user.name=V14",
            "-c",
            "user.email=v14@example.invalid",
            "commit",
            "-q",
            "-m",
            "three",
        ],
    );

    let checked_out = git(&ahead, &["push", work.to_str().unwrap(), "main"]);
    assert!(
        !checked_out.status.success(),
        "pushing the checked-out branch must be refused"
    );
    assert!(
        String::from_utf8_lossy(&checked_out.stderr)
            .contains("refusing to update checked out branch"),
        "expected `refusing to update checked out branch`, got {:?}",
        String::from_utf8_lossy(&checked_out.stderr)
    );
    let other = git(&ahead, &["push", work.to_str().unwrap(), "main:other"]);
    assert!(
        other.status.success(),
        "a non-checked-out branch in the same non-bare repo must be updatable: {}",
        String::from_utf8_lossy(&other.stderr)
    );
}

fn backend_post(root: &Path, path_info: &str) -> Output {
    Command::new("git")
        .arg("http-backend")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path_info)
        .env("QUERY_STRING", "")
        .env("REQUEST_METHOD", "POST")
        .env("CONTENT_TYPE", "application/x-git-receive-pack-request")
        .env("CONTENT_LENGTH", "4")
        .env("REMOTE_ADDR", "127.0.0.1")
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(b"0000")?;
            child.wait_with_output()
        })
        .expect("run git http-backend receive-pack probe")
}
