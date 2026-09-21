//! V14 —— 独立验证 T14（smart HTTP 传输）。
//!
//! 这个 target **真的起一个 TCP HTTP 服务器**（本机 `127.0.0.1:0`，系统分配端口并打印），
//! 把请求转发给真实 `git http-backend`（CGI），再用 `HttpRemote` 走真实 TCP 去请求它。
//! `git` 不是被测对象，而是不可作弊的 oracle：advertisement、pack、report-status
//! 全部由真实 git 产出/校验。
//!
//! 脚手架故意提供几种坏响应（404/500、无 framing、连接即断、纯文本），用来验证
//! 客户端给出清晰 `Err` 而不是 panic / 死循环。

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use minigit::transport::http::HttpRemote;
use minigit::transport::negotiate::{
    build_fetch_request, build_push_update, parse_advertisement, parse_report_status, FetchRequest,
    PushCommand,
};
use minigit::transport::pktline::{read_pkt, Pkt};
use minigit::{Error, Oid};

// ---------------------------------------------------------------------------
// 脚手架：TCP HTTP 服务器 + git http-backend CGI
// ---------------------------------------------------------------------------

/// 脚手架可以故意制造的响应形态。
#[derive(Clone, Copy, Debug)]
enum Fault {
    /// 真实转发 `git http-backend`，用 `Content-Length` framing 回。
    Normal,
    /// 真实转发，但用 `Transfer-Encoding: chunked` 分片回。
    Chunked,
    /// 直接回一个非 200 状态码。
    Status(u16, &'static str),
    /// 回 200 但既没有 `Content-Length` 也没有 chunked。
    NoFraming,
    /// 回纯文本 `hello`，根本不是 HTTP。
    PlainText,
    /// accept 之后立刻关闭连接。
    CloseImmediately,
}

/// 客户端实际发出的 request line + headers（脚手架端记录，用于逐字节对拍）。
#[derive(Clone, Debug)]
struct RecordedRequest {
    head: Vec<u8>,
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl RecordedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

struct TestServer {
    addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    records: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl TestServer {
    fn start(root: PathBuf, fault: Fault) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local_addr");
        eprintln!(
            "verify_http: scaffold listening on http://{addr} (fault={fault:?}, root={})",
            root.display()
        );
        listener
            .set_nonblocking(true)
            .expect("set listener nonblocking");
        let shutdown = Arc::new(AtomicBool::new(false));
        let records: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let flag = Arc::clone(&shutdown);
        let recs = Arc::clone(&records);
        let join = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = handle_connection(stream, &root, fault, &recs);
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        TestServer {
            addr,
            shutdown,
            join: Some(join),
            records,
        }
    }

    fn base_url(&self, repo: &str) -> String {
        format!("http://{}/{repo}", self.addr)
    }

    /// 客户端返回时服务端线程可能还没 push 记录：轮询等待，最多 5s（不静默跳过）。
    fn request(&self, index: usize) -> RecordedRequest {
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

impl Drop for TestServer {
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
    records: &Mutex<Vec<RecordedRequest>>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    if let Fault::CloseImmediately = fault {
        drop(stream);
        return Ok(());
    }

    let mut reader = BufReader::new(stream.try_clone()?);

    let request_line = read_http_request_line(&mut reader)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut raw_head = Vec::new();
    raw_head.extend_from_slice(request_line.as_bytes());
    raw_head.extend_from_slice(b"\r\n");

    let mut content_length = 0usize;
    let mut content_type: Option<String> = None;
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let header = read_http_request_line(&mut reader)?;
        if header.is_empty() {
            break;
        }
        raw_head.extend_from_slice(header.as_bytes());
        raw_head.extend_from_slice(b"\r\n");
        if let Some((name, value)) = header.split_once(':') {
            let name = name.trim().to_string();
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            } else if name.eq_ignore_ascii_case("content-type") {
                content_type = Some(value.clone());
            }
            headers.push((name, value));
        }
    }
    raw_head.extend_from_slice(b"\r\n");
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    records
        .lock()
        .expect("records mutex")
        .push(RecordedRequest {
            head: raw_head,
            method: method.clone(),
            target: target.clone(),
            headers,
        });

    match fault {
        Fault::PlainText => {
            stream.write_all(b"hello")?;
            stream.flush()?;
            return Ok(());
        }
        Fault::Status(code, reason) => {
            let text = format!("scaffold fault body for HTTP {code}");
            let head = format!(
                "HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                text.len()
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(text.as_bytes())?;
            stream.flush()?;
            return Ok(());
        }
        Fault::Normal | Fault::Chunked | Fault::NoFraming | Fault::CloseImmediately => {}
    }

    let (path_info, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), query.to_string()),
        None => (target.clone(), String::new()),
    };
    let body_arg = if content_length > 0 {
        Some(body.as_slice())
    } else {
        None
    };
    let cgi = run_backend(
        root,
        &method,
        &path_info,
        &query,
        content_type.as_deref(),
        body_arg,
    );
    let (status, cgi_content_type, cgi_body) = parse_cgi(&cgi);
    let reason = reason_phrase(status);

    match fault {
        Fault::Normal => write_framed(
            &mut stream,
            status,
            reason,
            cgi_content_type.as_deref(),
            &cgi_body,
            false,
        )?,
        Fault::Chunked => write_framed(
            &mut stream,
            status,
            reason,
            cgi_content_type.as_deref(),
            &cgi_body,
            true,
        )?,
        Fault::NoFraming => {
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(&cgi_body)?;
            stream.flush()?;
        }
        Fault::PlainText | Fault::Status(..) | Fault::CloseImmediately => unreachable!(),
    }
    Ok(())
}

fn read_http_request_line<R: BufRead>(reader: &mut R) -> std::io::Result<String> {
    let mut buf = Vec::new();
    let read = reader.read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Err(std::io::Error::new(
            ErrorKind::UnexpectedEof,
            "client closed connection",
        ));
    }
    while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
        buf.pop();
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn write_framed(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: Option<&str>,
    body: &[u8],
    chunked: bool,
) -> std::io::Result<()> {
    let content_type = content_type.unwrap_or("application/octet-stream");
    if chunked {
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(head.as_bytes())?;
        // 故意拆成多个小 chunk，确保客户端真的在做分块解码而不是碰巧一次读完。
        for piece in body.chunks(37) {
            stream.write_all(format!("{:x}\r\n", piece.len()).as_bytes())?;
            stream.write_all(piece)?;
            stream.write_all(b"\r\n")?;
        }
        stream.write_all(b"0\r\n\r\n")?;
    } else {
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes())?;
        stream.write_all(body)?;
    }
    stream.flush()
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

/// 把一次请求交给真实 `git http-backend`（CGI）。所有环境变量都用 `Command::env`
/// 就地传入，绝不 export 到整个 shell。
fn run_backend(
    root: &Path,
    method: &str,
    path_info: &str,
    query: &str,
    content_type: Option<&str>,
    body: Option<&[u8]>,
) -> Vec<u8> {
    let mut cmd = Command::new("git");
    cmd.arg("http-backend")
        .env("GIT_PROJECT_ROOT", root)
        .env("PATH_INFO", path_info)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", method)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_PROTOCOL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(content_type) = content_type {
        cmd.env("CONTENT_TYPE", content_type);
    }
    if let Some(body) = body {
        cmd.env("CONTENT_LENGTH", body.len().to_string());
    }
    let mut child = cmd.spawn().expect("git http-backend must be on PATH");
    if let Some(body) = body {
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(body)
            .expect("write body to git http-backend");
    } else {
        drop(child.stdin.take());
    }
    let output = child.wait_with_output().expect("wait for git http-backend");
    assert!(
        output.status.success(),
        "git http-backend failed (status {}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// 解析 CGI 输出：header 块（`\r\n\r\n` 或 `\n\n` 分隔）+ body，读 `Status:` 与
/// `Content-Type` 头。真实 git 客户端会校验响应 Content-Type，故必须原样转发。
fn parse_cgi(raw: &[u8]) -> (u16, Option<String>, Vec<u8>) {
    let split = find_subslice(raw, b"\r\n\r\n")
        .map(|index| (index, 4))
        .or_else(|| find_subslice(raw, b"\n\n").map(|index| (index, 2)));
    let (head, body) = match split {
        Some((index, separator)) => (&raw[..index], raw[index + separator..].to_vec()),
        None => (raw, Vec::new()),
    };

    let mut status = 200u16;
    let mut content_type = None;
    let head = String::from_utf8_lossy(head);
    for line in head.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(value) = line.strip_prefix("Status:") {
            if let Some(code) = value.split_whitespace().next() {
                status = code.parse().unwrap_or(200);
            }
        } else if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-type") {
                content_type = Some(value.trim().to_string());
            }
        }
    }
    (status, content_type, body)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ---------------------------------------------------------------------------
// 真值与夹具
// ---------------------------------------------------------------------------

fn git(cwd: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git must be installed and on PATH")
}

fn git_ok(cwd: &Path, args: &[&str]) -> Output {
    let output = git(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}\n{}",
        cwd.display(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    output
}

fn git_commit(cwd: &Path, message: &str) {
    let output = Command::new("git")
        .args(["commit", "-qm", message])
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t14")
        .env("GIT_AUTHOR_EMAIL", "t14@example.com")
        .env("GIT_COMMITTER_NAME", "t14")
        .env("GIT_COMMITTER_EMAIL", "t14@example.com")
        .env("GIT_AUTHOR_DATE", "1700000000 +0800")
        .env("GIT_COMMITTER_DATE", "1700000000 +0800")
        .output()
        .expect("git commit");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// 建一个 bare 仓库 `srv.git`（含一个提交），并允许 HTTP push。
fn make_bare_server(root: &Path) -> PathBuf {
    let seed = root.join("seed");
    std::fs::create_dir_all(&seed).expect("create seed dir");
    git_ok(&seed, &["init", "-q", "-b", "main", "."]);
    std::fs::write(seed.join("f.txt"), b"one\n").expect("write seed file");
    git_ok(&seed, &["add", "f.txt"]);
    git_commit(&seed, "c1");

    git_ok(root, &["clone", "-q", "--bare", "seed", "srv.git"]);
    let bare = root.join("srv.git");
    git_ok(&bare, &["config", "http.receivepack", "true"]);
    bare
}

fn rev_parse(cwd: &Path, rev: &str) -> String {
    String::from_utf8(git_ok(cwd, &["rev-parse", rev]).stdout)
        .expect("rev-parse output is UTF-8")
        .trim()
        .to_string()
}

fn remote_for(server: &TestServer, repo: &str) -> HttpRemote {
    HttpRemote::new(&server.base_url(repo))
}

// ---------------------------------------------------------------------------
// 硬标准
// ---------------------------------------------------------------------------

#[test]
fn hard_standard_info_refs_advertisement_matches_real_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bare = make_bare_server(tmp.path());
    let server = TestServer::start(tmp.path().to_path_buf(), Fault::Normal);

    let advertisement_bytes = remote_for(&server, "srv.git")
        .info_refs("git-upload-pack")
        .expect("info_refs must succeed against a real git http-backend");
    let advertisement =
        parse_advertisement(&advertisement_bytes).expect("HTTP advertisement must parse");

    // 独立真值 1：真实 git 直接输出 advertisement。
    let expected_raw = git_ok(
        &bare,
        &[
            "-c",
            "protocol.version=0",
            "upload-pack",
            "--advertise-refs",
            ".",
        ],
    )
    .stdout;
    let expected = parse_advertisement(&expected_raw).expect("real git advertisement must parse");

    let mut actual_refs = advertisement.refs.clone();
    actual_refs.sort();
    let mut expected_refs = expected.refs.clone();
    expected_refs.sort();
    assert_eq!(
        actual_refs, expected_refs,
        "HTTP advertisement refs must match `git upload-pack --advertise-refs`"
    );

    // 独立真值 2：for-each-ref / rev-parse。
    let main_oid = rev_parse(&bare, "refs/heads/main");
    assert!(
        advertisement
            .refs
            .iter()
            .any(|(name, oid)| name == "refs/heads/main" && oid.to_string() == main_oid),
        "advertisement must contain refs/heads/main -> {main_oid}, got {:?}",
        advertisement.refs
    );
    assert!(
        advertisement.refs.iter().any(|(name, _)| name == "HEAD"),
        "advertisement must include HEAD, got {:?}",
        advertisement.refs
    );
    assert!(
        advertisement
            .symrefs
            .iter()
            .any(|(name, target)| name == "HEAD" && target == "refs/heads/main"),
        "advertisement must carry symref=HEAD:refs/heads/main, got {:?}",
        advertisement.symrefs
    );
}

#[test]
fn hard_standard_upload_pack_pack_unpacks_with_real_git() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _bare = make_bare_server(tmp.path());
    let server = TestServer::start(tmp.path().to_path_buf(), Fault::Normal);
    let remote = remote_for(&server, "srv.git");

    let advertisement = parse_advertisement(
        &remote
            .info_refs("git-upload-pack")
            .expect("info_refs must succeed"),
    )
    .expect("advertisement must parse");
    let head = advertisement
        .refs
        .iter()
        .find(|(name, _)| name == "HEAD")
        .or_else(|| {
            advertisement
                .refs
                .iter()
                .find(|(name, _)| name == "refs/heads/main")
        })
        .map(|(_, oid)| *oid)
        .expect("advertisement must contain HEAD or refs/heads/main");

    let request = FetchRequest {
        wants: vec![head],
        haves: Vec::new(),
        done: true,
    };
    let body = build_fetch_request(&request, &["side-band-64k"]);
    let response = remote
        .upload_pack(&body)
        .expect("upload_pack must succeed against git http-backend");

    let mut cursor = std::io::Cursor::new(response.as_slice());
    let mut pack = Vec::new();
    let mut saw_status = false;
    loop {
        match read_pkt(&mut cursor).expect("upload-pack response must be valid pkt-line") {
            Pkt::Data(data) => {
                if data.starts_with(b"NAK") || data.starts_with(b"ACK") {
                    saw_status = true;
                } else if data.first() == Some(&1) {
                    pack.extend_from_slice(&data[1..]);
                } else if data.first() == Some(&3) {
                    panic!(
                        "server reported a side-band error: {}",
                        String::from_utf8_lossy(&data[1..])
                    );
                }
            }
            Pkt::Flush => break,
            other => panic!("unexpected frame in upload-pack response: {other:?}"),
        }
    }
    assert!(saw_status, "upload-pack response must start with NAK/ACK");
    assert!(!pack.is_empty(), "side-band channel 1 must carry pack data");

    // 真值：真实 `git unpack-objects -q` 必须 exit 0。
    let dst = tmp.path().join("dst");
    git_ok(tmp.path(), &["init", "-q", "-b", "main", "dst"]);
    let mut child = Command::new("git")
        .args(["unpack-objects", "-q"])
        .current_dir(&dst)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn git unpack-objects");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(&pack)
        .expect("write pack to git unpack-objects");
    let output = child.wait_with_output().expect("wait git unpack-objects");
    assert!(
        output.status.success(),
        "git unpack-objects rejected the fetched pack: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let head_hex = head.to_string();
    let kind = git_ok(&dst, &["cat-file", "-t", &head_hex]);
    assert_eq!(
        String::from_utf8_lossy(&kind.stdout).trim(),
        "commit",
        "the fetched HEAD commit must exist after git unpack-objects"
    );
}

#[test]
fn hard_standard_receive_pack_returns_report_status() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bare = make_bare_server(tmp.path());

    git_ok(tmp.path(), &["clone", "-q", "srv.git", "cli"]);
    let cli = tmp.path().join("cli");
    std::fs::write(cli.join("f.txt"), b"two\n").expect("write cli file");
    git_ok(&cli, &["add", "f.txt"]);
    git_commit(&cli, "c2");

    let old = rev_parse(&bare, "refs/heads/main");
    let new = rev_parse(&cli, "refs/heads/main");
    assert_ne!(old, new, "fixture must create a new commit to push");

    let commands = vec![PushCommand {
        old: Oid::from_hex(&old).expect("old oid"),
        new: Oid::from_hex(&new).expect("new oid"),
        name: "refs/heads/main".to_string(),
    }];
    let mut body = build_push_update(&commands, &["report-status"]).expect("build_push_update");
    let pack = git_ok(&cli, &["pack-objects", "--stdout", "--all"]).stdout;
    assert!(!pack.is_empty(), "pack-objects must produce a pack");
    body.extend_from_slice(&pack);

    let server = TestServer::start(tmp.path().to_path_buf(), Fault::Normal);
    let response = remote_for(&server, "srv.git")
        .receive_pack(&body)
        .expect("receive_pack must succeed against git http-backend");

    let statuses = parse_report_status(&response).expect("report-status must parse");
    assert!(
        !statuses.is_empty(),
        "report-status must not be empty: {statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .any(|(what, status)| what == "unpack" && status == "ok"),
        "expected `unpack ok`, got {statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .any(|(name, status)| name == "refs/heads/main" && status == "ok"),
        "expected ok for refs/heads/main, got {statuses:?}"
    );

    assert_eq!(
        rev_parse(&bare, "refs/heads/main"),
        new,
        "the server ref must have been updated over HTTP push"
    );
}

#[test]
fn hard_standard_chunked_response_is_decoded() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _bare = make_bare_server(tmp.path());
    let normal = TestServer::start(tmp.path().to_path_buf(), Fault::Normal);
    let chunked = TestServer::start(tmp.path().to_path_buf(), Fault::Chunked);

    let plain = remote_for(&normal, "srv.git")
        .info_refs("git-upload-pack")
        .expect("Content-Length response");
    let decoded = remote_for(&chunked, "srv.git")
        .info_refs("git-upload-pack")
        .expect("chunked response must decode");

    assert_eq!(
        plain, decoded,
        "chunked-decoded body must equal the Content-Length body byte for byte"
    );
    let advertisement = parse_advertisement(&decoded).expect("decoded advertisement must parse");
    assert!(
        advertisement
            .refs
            .iter()
            .any(|(name, _)| name == "refs/heads/main"),
        "decoded advertisement must list the branch"
    );
}

// ---------------------------------------------------------------------------
// 反例：必须是清晰 Err，不许 panic / 死循环
// ---------------------------------------------------------------------------

#[test]
fn http_error_statuses_surface_code_and_body() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _bare = make_bare_server(tmp.path());

    for (code, reason) in [(404u16, "Not Found"), (500u16, "Internal Server Error")] {
        let server = TestServer::start(tmp.path().to_path_buf(), Fault::Status(code, reason));
        let err = remote_for(&server, "srv.git")
            .info_refs("git-upload-pack")
            .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains(&code.to_string()),
            "error must carry the HTTP status {code}: {text}"
        );
        assert!(
            text.contains("scaffold fault body"),
            "error must include a body snippet for debugging: {text}"
        );
    }
}

#[test]
fn missing_framing_is_a_clear_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _bare = make_bare_server(tmp.path());
    let server = TestServer::start(tmp.path().to_path_buf(), Fault::NoFraming);

    let err = remote_for(&server, "srv.git")
        .info_refs("git-upload-pack")
        .unwrap_err();
    assert!(
        matches!(err, Error::Protocol(_)),
        "expected a Protocol error, got {err:?}"
    );
    assert!(
        err.to_string().contains("Content-Length"),
        "message should explain the missing framing: {err}"
    );
}

#[test]
fn non_http_response_is_a_clear_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _bare = make_bare_server(tmp.path());
    let server = TestServer::start(tmp.path().to_path_buf(), Fault::PlainText);

    let err = remote_for(&server, "srv.git")
        .info_refs("git-upload-pack")
        .unwrap_err();
    assert!(
        matches!(err, Error::Protocol(_)),
        "expected a Protocol error for a non-HTTP response, got {err:?}"
    );
}

#[test]
fn closed_connection_is_a_clear_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _bare = make_bare_server(tmp.path());
    let server = TestServer::start(tmp.path().to_path_buf(), Fault::CloseImmediately);

    let err = remote_for(&server, "srv.git")
        .info_refs("git-upload-pack")
        .unwrap_err();
    assert!(
        !err.to_string().is_empty(),
        "closed connection must produce an informative error"
    );
}

#[test]
fn https_is_unsupported() {
    let remote = HttpRemote::new("https://example.com/srv.git");
    let err = remote.info_refs("git-upload-pack").unwrap_err();
    assert!(
        matches!(err, Error::Unsupported(_)),
        "expected Unsupported for https://, got {err:?}"
    );
    assert!(
        remote.upload_pack(b"0000").is_err(),
        "upload_pack must also reject https://"
    );
}

// ---------------------------------------------------------------------------
// T14b：GET `info/refs` 的 `Accept` 与真实 git 逐字节一致
// ---------------------------------------------------------------------------

#[test]
fn info_refs_accept_header_matches_real_git_byte_for_byte() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _bare = make_bare_server(tmp.path());
    let server = TestServer::start(tmp.path().to_path_buf(), Fault::Normal);
    let url = server.base_url("srv.git");

    // 真值：真实 git 客户端（强制 v0）自己发出的 GET /info/refs。
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
    let git_get = server.request(0);

    let remote = remote_for(&server, "srv.git");
    let adv_bytes = remote
        .info_refs("git-upload-pack")
        .expect("info_refs(git-upload-pack) must succeed");
    let our_get = server.request(1);
    remote
        .info_refs("git-receive-pack")
        .expect("info_refs(git-receive-pack) must succeed");
    let our_push_get = server.request(2);

    eprintln!(
        "T14b raw GET /info/refs (HttpRemote):\n{}",
        String::from_utf8_lossy(&our_get.head)
    );
    eprintln!(
        "T14b raw GET /info/refs (real git):\n{}",
        String::from_utf8_lossy(&git_get.head)
    );

    assert_eq!(our_get.method, "GET");
    assert_eq!(our_get.target, git_get.target, "target must match real git");
    assert_eq!(
        our_get.header("Accept"),
        Some("*/*"),
        "GET info/refs Accept must be `*/*` like real git"
    );
    assert_eq!(
        our_get.header("Accept"),
        git_get.header("Accept"),
        "our GET Accept must match real git byte for byte"
    );
    assert_eq!(our_get.header("Host"), git_get.header("Host"));
    assert_eq!(our_get.header("User-Agent"), git_get.header("User-Agent"));
    assert_eq!(
        our_get.header("Content-Type"),
        None,
        "GET /info/refs must not send a Content-Type"
    );
    assert_eq!(
        our_push_get.target,
        "/srv.git/info/refs?service=git-receive-pack"
    );
    assert_eq!(
        our_push_get.header("Accept"),
        Some("*/*"),
        "receive-pack GET must also use `*/*`"
    );
    assert!(
        !our_get.header("Accept").unwrap().contains("git-git-"),
        "the T14b malformed `git-git-` Accept must be gone"
    );

    // POST 侧是非目标：必须保持 `...-result`（实测与真实 git 相同）。
    let advertisement = parse_advertisement(&adv_bytes).expect("advertisement must parse");
    let head = advertisement
        .refs
        .iter()
        .find(|(name, _)| name == "HEAD")
        .or_else(|| {
            advertisement
                .refs
                .iter()
                .find(|(name, _)| name == "refs/heads/main")
        })
        .map(|(_, oid)| *oid)
        .expect("advertisement must contain HEAD or refs/heads/main");
    let body = build_fetch_request(
        &FetchRequest {
            wants: vec![head],
            haves: vec![],
            done: true,
        },
        &["side-band-64k"],
    );
    let _ = remote.upload_pack(&body);
    let our_post = server.request(3);
    assert_eq!(our_post.method, "POST");
    eprintln!(
        "T14b raw POST /git-upload-pack (HttpRemote):\n{}",
        String::from_utf8_lossy(&our_post.head)
    );
    assert_eq!(
        our_post.header("Accept"),
        Some("application/x-git-upload-pack-result"),
        "POST Accept must stay unchanged"
    );
}
