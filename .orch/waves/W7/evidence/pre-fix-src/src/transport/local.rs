//! `file://` 传输（本地路径直读对端仓库）。**T13（codex）实现范围**。
//!
//! 实现方式：直接把对端仓库当作一个 `Repo` 打开，复用本地对象读取与 refs，
//! 但**必须走 pkt-line 编解码路径**（而不是直接调用本地 API），
//! 这样 T13 的产出的协商逻辑才能在 T14（HTTP）里原样复用。
//!
//! # 结构（为什么长这样）
//!
//! ```text
//!  cli/{clone,fetch,push,pull}        ── 只做参数与打印
//!        │
//!  transport::local::{fetch,push,clone_into}
//!        │  connect(url) -> dyn Connection
//!        ├── FileConn  ：进程内的 v0 服务端（LocalRemote）
//!        │       advertise()  ── 自己按 v0 语义生成 ref advertisement 字节
//!        │       rpc()        ── 自己生成 pack 与 side-band / report-status 字节
//!        └── HttpConn  ：T14 的 `HttpRemote`（本文件只路由，不实现）
//! ```
//!
//! 「客户端」一侧只吃字节：`negotiate::parse_advertisement` → `build_fetch_request` /
//! `build_push_update` → `negotiate::parse_report_status`，与真实 git 服务端产出的字节
//! 完全同构（`src/transport/negotiate.rs` 底部的差分测试用真实 git 进程验证了这一点）。
//! 因此 T14 只要把 `info_refs` / `upload_pack` / `receive_pack` 三个端点接通即可复用同一套协商。
//!
//! # 真实 git 语义对齐（实测，git 2.55.0）
//!
//! * 广告：`<oid> HEAD\0<caps>` + 全部引用（含 `refs/tags/x^{}` 剥离行）+ flush；
//!   空仓库是 `<40 个 0> capabilities^{}\0<caps>`（没有 `symref=`）。
//! * fetch 响应：`NAK` 之后是 side-band 通道 1 的 pack、通道 2 的进度，最后 flush。
//! * 空远端：`clone` exit 0、只建仓库（remote/branch 配置 + HEAD 符号引用）并打
//!   `warning: You appear to have cloned an empty repository.`（与真实 git 逐字相同），
//!   不 fetch、不检出；`fetch` exit 0、一个引用都不写，只把 `FETCH_HEAD` 截成空文件
//!   （真实 git 同）。
//! * push 的 pack **不打分帧**：update 段 flush 之后紧跟裸 pack。
//! * **non-fast-forward 由客户端拒绝**（`git push` 就是这么做的：`send-pack` 本地先判定，
//!   根本不发请求）；服务端默认不判 FF（`receive.denyNonFastForwards=false`），
//!   但**拒绝更新非 bare 远端当前检出的分支**（`receive.denyCurrentBranch=refuse`）——
//!   本实现两者都照做。
//!
//! # 已知限制（写进 result）
//!
//! * `file://` 服务端产出的 pack **自包含、不含 delta**；因此 `REF_DELTA` 需要 `.idx`
//!   才能找到 base（thin pack）这一点只在**接收**侧遇到，会明确报
//!   `Unsupported("REF_DELTA ...")`，不会静默出错。
//! * `push` 的 ref 删除（`:refs/heads/x`）、`--mirror`、`--tags`、`--all` 未实现。
//! * 非 bare 远端配置了 `receive.denyCurrentBranch=updateInstead` 时本实现仍然拒绝
//!   （真实 git 会顺带更新工作区）。
//! * 多 ref 的 push 是**全有或全无**：真实 git 会推成功一部分、拒绝另一部分。
//! * `https://` 报 `Unsupported`，`git://` 亦同（v1 不做 TCP/TLS）。

use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::object::Object;
use crate::odb::pack::{PackFile, PackSet};
use crate::odb::Odb;
use crate::oid::Oid;
use crate::refs::{Head, RefStore, HEADS_PREFIX, REMOTES_PREFIX, TAGS_PREFIX};
use crate::repo::Repo;
use crate::transport::http::HttpRemote;
use crate::transport::negotiate::{
    self, FetchRequest, PushCommand, RefAdvertisement, CAPABILITIES_REF,
};
use crate::transport::pktline::{read_pkt, write_flush, write_pkt, Pkt};
use crate::worktree::materialize::{checkout_to, HeadTarget};
use crate::zlib;

/// 默认远端名（`mg clone` 写进配置的那个）。
pub const DEFAULT_REMOTE: &str = "origin";

/// `mg clone` 默认写入的 fetch refspec（与真实 git 一致）。
pub const DEFAULT_FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";

/// side-band pkt 里每个包的最大数据字节数（1 字节通道号 + 65515 = 65516 = git 的上限）。
const SIDE_BAND_CHUNK: usize = 65515;

#[derive(Debug, Clone, Default)]
pub struct FetchOutcome {
    /// 更新后的远端引用（目标仓库中的名字，如 `refs/remotes/origin/main`）。
    pub refs: Vec<(String, Oid)>,
    pub head: Option<(String, Oid)>,
    pub objects_written: usize,
}

// ---------------------------------------------------------------- URL

/// 远端地址的解析结果。
enum Location {
    File(PathBuf),
    Http(String),
}

fn locate(url: &str) -> Result<Location> {
    if let Some(rest) = url.strip_prefix("file://") {
        let path = if let Some(local) = rest.strip_prefix("localhost") {
            local
        } else if rest.starts_with('/') {
            rest
        } else {
            return Err(Error::Unsupported(
                "file:// URLs with a remote host are not supported in v1",
            ));
        };
        return Ok(Location::File(PathBuf::from(percent_decode(path)?)));
    }
    if url.starts_with("http://") {
        return Ok(Location::Http(url.to_string()));
    }
    if url.starts_with("https://") {
        return Err(Error::Unsupported(
            "https:// is not supported in v1 (no TLS dependency)",
        ));
    }
    if let Some(scheme_end) = url.find("://") {
        return match &url[..scheme_end] {
            "git" => Err(Error::Unsupported(
                "the git:// (TCP) protocol is not supported in v1",
            )),
            "ssh" => Err(Error::Unsupported("ssh:// is not supported in v1")),
            other => Err(Error::Other(format!("unsupported URL scheme '{other}://'"))),
        };
    }
    if url.is_empty() {
        return Err(Error::Other("empty remote URL".to_string()));
    }
    // 没有 scheme 的一律按本地路径处理（真实 git 也是这样）。
    Ok(Location::File(PathBuf::from(percent_decode(url)?)))
}

fn percent_decode(text: &str) -> Result<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            if idx + 2 >= bytes.len() {
                return Err(Error::Other(format!("bad percent-escape in URL: {text}")));
            }
            let hex = std::str::from_utf8(&bytes[idx + 1..idx + 3])
                .map_err(|_| Error::Other(format!("bad percent-escape in URL: {text}")))?;
            let value = u8::from_str_radix(hex, 16)
                .map_err(|_| Error::Other(format!("bad percent-escape in URL: {text}")))?;
            out.push(value);
            idx += 3;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8(out).map_err(|_| Error::Other(format!("URL is not valid UTF-8: {text}")))
}

// ---------------------------------------------------------------- 连接抽象

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Service {
    UploadPack,
    ReceivePack,
}

impl Service {
    fn name(self) -> &'static str {
        match self {
            Service::UploadPack => "git-upload-pack",
            Service::ReceivePack => "git-receive-pack",
        }
    }
}

/// 远端连接的抽象：`file://` 是进程内服务端，`http://` 是 T14 的 `HttpRemote`。
///
/// 协商逻辑只依赖这两个方法，所以 T13 写好的 fetch/push 流程对 T14 直接可用。
trait Connection {
    fn advertise(&self, service: Service) -> Result<Vec<u8>>;
    fn rpc(&self, service: Service, request: &[u8]) -> Result<Vec<u8>>;
}

struct FileConn {
    remote: Repo,
}

impl Connection for FileConn {
    fn advertise(&self, service: Service) -> Result<Vec<u8>> {
        LocalRemote { repo: &self.remote }.advertise(service)
    }

    fn rpc(&self, service: Service, request: &[u8]) -> Result<Vec<u8>> {
        let server = LocalRemote { repo: &self.remote };
        match service {
            Service::UploadPack => server.upload_pack(request),
            Service::ReceivePack => server.receive_pack(request),
        }
    }
}

struct HttpConn {
    http: HttpRemote,
}

impl Connection for HttpConn {
    fn advertise(&self, service: Service) -> Result<Vec<u8>> {
        self.http.info_refs(service.name())
    }

    fn rpc(&self, service: Service, request: &[u8]) -> Result<Vec<u8>> {
        match service {
            Service::UploadPack => self.http.upload_pack(request),
            Service::ReceivePack => self.http.receive_pack(request),
        }
    }
}

fn connect(url: &str) -> Result<Box<dyn Connection>> {
    match locate(url)? {
        Location::File(path) => {
            if path.as_os_str().is_empty() {
                return Err(Error::Other("empty remote path".to_string()));
            }
            if !path.exists() {
                return Err(Error::Other(format!(
                    "remote repository '{}' does not exist",
                    path.display()
                )));
            }
            let repo = Repo::discover(&path)?;
            Ok(Box::new(FileConn { remote: repo }))
        }
        Location::Http(url) => Ok(Box::new(HttpConn {
            http: HttpRemote::new(&url),
        })),
    }
}

// ---------------------------------------------------------------- 进程内 v0 服务端

/// 把本地仓库当成一个 v0 服务端：产出与真实 git 同构的字节流。
struct LocalRemote<'a> {
    repo: &'a Repo,
}

impl LocalRemote<'_> {
    fn upload_caps(&self) -> Result<Vec<String>> {
        let mut caps = vec![
            "multi_ack".to_string(),
            "thin-pack".to_string(),
            "side-band".to_string(),
            "side-band-64k".to_string(),
            "ofs-delta".to_string(),
            "no-progress".to_string(),
            "include-tag".to_string(),
        ];
        if let Ok(Head::Attached(target)) = RefStore::new(self.repo).read_head() {
            caps.push(format!("symref=HEAD:{target}"));
        }
        caps.push("object-format=sha1".to_string());
        caps.push(format!("agent=mg/{}", env!("CARGO_PKG_VERSION")));
        Ok(caps)
    }

    fn receive_caps(&self) -> Vec<String> {
        vec![
            "report-status".to_string(),
            "ofs-delta".to_string(),
            format!("agent=mg/{}", env!("CARGO_PKG_VERSION")),
        ]
    }

    /// v0 引用广告的完整字节流：`<oid> HEAD\0<caps>` + 全部引用 + flush。
    fn advertise(&self, service: Service) -> Result<Vec<u8>> {
        let caps = match service {
            Service::UploadPack => self.upload_caps()?,
            Service::ReceivePack => self.receive_caps(),
        };
        let refs = RefStore::new(self.repo);
        let head = refs.resolve("HEAD").ok().filter(|oid| !oid.is_zero());

        let mut out = Vec::new();
        let mut first = String::new();
        match head {
            Some(oid) => {
                first.push_str(&oid.to_hex());
                first.push_str(" HEAD");
            }
            None => {
                first.push_str(&Oid::zeros().to_hex());
                first.push(' ');
                first.push_str(CAPABILITIES_REF);
            }
        }
        first.push('\0');
        first.push_str(&caps.join(" "));
        first.push('\n');
        write_pkt(&mut out, first.as_bytes())?;

        let odb = Odb::new(self.repo);
        for (name, oid) in refs.list()? {
            write_pkt(&mut out, format!("{} {name}\n", oid.to_hex()).as_bytes())?;
            if name.starts_with(TAGS_PREFIX) {
                if let Ok(Object::Tag(tag)) = odb.read_object(oid) {
                    // 真实 git 会把附注 tag 剥到最终对象，并紧跟一行 `<peeled> refs/tags/x^{}`。
                    let mut target = tag.object;
                    for _ in 0..8 {
                        match odb.read_object(target) {
                            Ok(Object::Tag(inner)) => target = inner.object,
                            _ => break,
                        }
                    }
                    write_pkt(
                        &mut out,
                        format!("{} {name}^{{}}\n", target.to_hex()).as_bytes(),
                    )?;
                }
            }
        }
        write_flush(&mut out)?;
        Ok(out)
    }

    /// 处理 fetch 请求，返回 `NAK` + side-band(pack) 的响应字节。
    fn upload_pack(&self, request: &[u8]) -> Result<Vec<u8>> {
        let parsed = parse_fetch_request(request)?;
        if parsed.wants.is_empty() {
            return Err(Error::Protocol("upload-pack: no `want` lines".into()));
        }
        let odb = Odb::new(self.repo);
        for want in &parsed.wants {
            if odb.read(*want).is_err() {
                return Err(Error::Protocol(format!(
                    "upload-pack: not our ref {}",
                    want.to_hex()
                )));
            }
        }

        // haves 只是「客户端已有」的提示：认不出的直接忽略（真实 git 同样只 ACK 公共对象）。
        let mut haves = Vec::new();
        for have in &parsed.haves {
            if !have.is_zero() && odb.read(*have).is_ok() {
                haves.push(*have);
            }
        }
        let known = reachable(self.repo, &haves, true)?;
        let mut send: BTreeSet<Oid> = BTreeSet::new();
        for want in &parsed.wants {
            for oid in reachable(self.repo, &[*want], false)? {
                if !known.contains(&oid) {
                    send.insert(oid);
                }
            }
        }

        let pack = build_pack(self.repo, &send)?;
        let progress = format!("Enumerating objects: {}, done.\n", send.len());

        let mut out = Vec::new();
        write_pkt(&mut out, b"NAK\n")?;
        if !parsed.no_progress {
            for chunk in progress.as_bytes().chunks(SIDE_BAND_CHUNK) {
                let mut payload = Vec::with_capacity(chunk.len() + 1);
                payload.push(2);
                payload.extend_from_slice(chunk);
                write_pkt(&mut out, &payload)?;
            }
        }
        for chunk in pack.chunks(SIDE_BAND_CHUNK) {
            let mut payload = Vec::with_capacity(chunk.len() + 1);
            payload.push(1);
            payload.extend_from_slice(chunk);
            write_pkt(&mut out, &payload)?;
        }
        write_flush(&mut out)?;
        Ok(out)
    }

    /// 处理 push 请求（update 段 + 裸 pack），把对象与引用真正写进远端，返回 report-status 字节。
    fn receive_pack(&self, request: &[u8]) -> Result<Vec<u8>> {
        let (cmds, pack) = parse_push_request(request)?;
        let refs = RefStore::new(self.repo);

        // 1) 先做全部校验，**一个字节都不写**，这样被拒绝时远端保证原样。
        let mut rejections: Vec<(String, String)> = Vec::new();
        for cmd in &cmds {
            let current = refs.resolve(&cmd.name).ok().unwrap_or_else(Oid::zeros);
            if current != cmd.old {
                rejections.push((
                    cmd.name.clone(),
                    "cannot lock ref: the remote ref moved".to_string(),
                ));
                continue;
            }
            if let Some(branch) = self.checked_out_branch() {
                if cmd.name == format!("{HEADS_PREFIX}{branch}") {
                    rejections.push((
                        cmd.name.clone(),
                        "branch is currently checked out".to_string(),
                    ));
                }
            }
        }
        if !rejections.is_empty() {
            let mut out = Vec::new();
            write_pkt(&mut out, b"unpack ok\n")?;
            for cmd in &cmds {
                match rejections.iter().find(|(name, _)| *name == cmd.name) {
                    Some((_, reason)) => {
                        write_pkt(&mut out, format!("ng {} {reason}\n", cmd.name).as_bytes())?
                    }
                    None => write_pkt(
                        &mut out,
                        format!("ng {} push aborted\n", cmd.name).as_bytes(),
                    )?,
                }
            }
            write_flush(&mut out)?;
            return Ok(out);
        }

        // 2) 落盘对象：pack 里的每个对象都写成 loose（远端是真实 git 仓库也照样能读）。
        let unpack_error = if pack.is_empty() {
            let odb = Odb::new(self.repo);
            cmds.iter()
                .find(|cmd| !cmd.new.is_zero() && odb.read(cmd.new).is_err())
                .map(|cmd| format!("missing objects for {}", cmd.name))
        } else {
            install_pack(self.repo, pack)
                .err()
                .map(|err| err.to_string())
        };
        if let Some(detail) = unpack_error.clone() {
            let mut out = Vec::new();
            write_pkt(&mut out, format!("unpack {detail}\n").as_bytes())?;
            write_flush(&mut out)?;
            return Ok(out);
        }

        // 3) 更新引用（CAS：必须与客户端声明的 old 一致）。
        let mut out = Vec::new();
        write_pkt(&mut out, b"unpack ok\n")?;
        for cmd in &cmds {
            // 创建（old = 全 0）要求引用不存在；其余是 CAS。
            let expected = if cmd.old.is_zero() {
                None
            } else {
                Some(Some(cmd.old))
            };
            let updated = refs.update(&cmd.name, cmd.new, expected);
            match updated {
                Ok(()) => write_pkt(&mut out, format!("ok {}\n", cmd.name).as_bytes())?,
                Err(err) => write_pkt(&mut out, format!("ng {} {err}\n", cmd.name).as_bytes())?,
            }
        }
        write_flush(&mut out)?;
        Ok(out)
    }

    /// 非 bare 仓库当前检出的分支短名（`receive.denyCurrentBranch=refuse` 用）。
    fn checked_out_branch(&self) -> Option<String> {
        self.repo.workdir()?;
        // bare 仓库的 HEAD 也是 attached，但它没有工作区：`core.bare=true`，
        // 或者 `Repo::discover` 把 bare 目录的 workdir 与 git_dir 指到了同一处。
        if self.repo.config().get_bool("core.bare").unwrap_or(false)
            || self.repo.workdir() == Some(self.repo.git_dir())
        {
            return None;
        }
        match RefStore::new(self.repo).read_head() {
            Ok(Head::Attached(name)) => name.strip_prefix(HEADS_PREFIX).map(str::to_string),
            _ => None,
        }
    }
}

/// 客户端 fetch 请求的解析结果。
struct ParsedFetch {
    wants: Vec<Oid>,
    haves: Vec<Oid>,
    no_progress: bool,
}

fn parse_fetch_request(bytes: &[u8]) -> Result<ParsedFetch> {
    let mut cursor = Cursor::new(bytes);
    let mut parsed = ParsedFetch {
        wants: Vec::new(),
        haves: Vec::new(),
        no_progress: false,
    };
    loop {
        match read_pkt(&mut cursor)? {
            Pkt::Data(data) => {
                let line = data.strip_suffix(b"\n").unwrap_or(&data);
                let text = std::str::from_utf8(line)
                    .map_err(|_| Error::Protocol("fetch request line is not UTF-8".into()))?;
                if let Some(rest) = text.strip_prefix("want ") {
                    let mut parts = rest.split(' ');
                    let oid = Oid::from_hex(parts.next().unwrap_or_default())?;
                    for cap in parts {
                        if cap == "no-progress" {
                            parsed.no_progress = true;
                        }
                    }
                    parsed.wants.push(oid);
                } else if let Some(rest) = text.strip_prefix("have ") {
                    parsed.haves.push(Oid::from_hex(rest)?);
                } else if text == "done" {
                    break;
                } else if text.starts_with("shallow ") || text.starts_with("deepen") {
                    return Err(Error::Unsupported(
                        "shallow fetch (deepen/shallow lines) is not supported in v1",
                    ));
                } else if !text.is_empty() {
                    return Err(Error::Protocol(format!(
                        "unrecognized fetch request line: {text:?}"
                    )));
                }
            }
            // want 与 have 之间的 flush 是分帧，不是结束。
            Pkt::Flush => continue,
            Pkt::Delim | Pkt::ResponseEnd => {
                return Err(Error::Protocol(
                    "unexpected delim/response-end pkt in a fetch request".into(),
                ))
            }
        }
    }
    Ok(parsed)
}

/// 客户端 push 请求：update 段 + 紧跟其后的裸 pack。
fn parse_push_request(bytes: &[u8]) -> Result<(Vec<PushCommand>, &[u8])> {
    let mut cursor = Cursor::new(bytes);
    let mut cmds = Vec::new();
    loop {
        match read_pkt(&mut cursor)? {
            Pkt::Data(data) => {
                let line = data.strip_suffix(b"\n").unwrap_or(&data);
                let (body, _caps) = match line.iter().position(|&b| b == 0) {
                    Some(idx) => (&line[..idx], Some(&line[idx + 1..])),
                    None => (line, None),
                };
                let text = std::str::from_utf8(body)
                    .map_err(|_| Error::Protocol("push command is not UTF-8".into()))?;
                let mut parts = text.split(' ');
                let old = Oid::from_hex(parts.next().unwrap_or_default())?;
                let new = Oid::from_hex(parts.next().unwrap_or_default())?;
                let name = parts.next().unwrap_or_default().to_string();
                if name.is_empty() {
                    return Err(Error::Protocol(format!(
                        "push command has no ref name: {text:?}"
                    )));
                }
                cmds.push(PushCommand { old, new, name });
            }
            Pkt::Flush => break,
            Pkt::Delim | Pkt::ResponseEnd => {
                return Err(Error::Protocol(
                    "unexpected delim/response-end pkt in a push request".into(),
                ))
            }
        }
    }
    if cmds.is_empty() {
        return Err(Error::Protocol("push request has no ref updates".into()));
    }
    let pack_start = cursor.position() as usize;
    Ok((cmds, &bytes[pack_start..]))
}

// ---------------------------------------------------------------- 对象遍历与 pack 生成

/// `roots` 可达的全部对象（commit/tree/blob/tag 闭包）。
///
/// `allow_missing` = 允许某个根对象在库里不存在（`haves` 提示用；真实 git 也忽略
/// 服务端不认识的 `have`）。为 false 时缺失即 `ObjectNotFound`。
fn reachable(repo: &Repo, roots: &[Oid], allow_missing: bool) -> Result<BTreeSet<Oid>> {
    let odb = Odb::new(repo);
    let mut seen = BTreeSet::new();
    let mut stack: Vec<Oid> = roots.to_vec();
    while let Some(oid) = stack.pop() {
        if !seen.insert(oid) {
            continue;
        }
        let object = match odb.read_object(oid) {
            Ok(object) => object,
            Err(Error::ObjectNotFound(_)) if allow_missing => continue,
            Err(err) => return Err(err),
        };
        match object {
            Object::Commit(commit) => {
                stack.push(commit.tree);
                stack.extend(commit.parents);
            }
            Object::Tree(tree) => stack.extend(tree.entries().iter().map(|entry| entry.oid)),
            Object::Tag(tag) => stack.push(tag.object),
            Object::Blob(_) => {}
        }
    }
    Ok(seen)
}

/// 生成一个 v0 可用的、**自包含且不含 delta** 的 pack。
///
/// 对象按 oid 升序写入（确定性输出），每个对象都是 `header + zlib(payload)`，
/// 因此接收侧不需要 `.idx` 就能枚举（`scan_pack_offsets`）与解码（`PackFile::read_at`）。
fn build_pack(repo: &Repo, oids: &BTreeSet<Oid>) -> Result<Vec<u8>> {
    let odb = Odb::new(repo);
    let mut out = Vec::new();
    out.extend_from_slice(b"PACK");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(oids.len() as u32).to_be_bytes());
    for oid in oids {
        let (kind, payload) = odb.read(*oid)?;
        write_object_header(&mut out, kind, payload.len() as u64);
        out.extend_from_slice(&zlib::deflate(&payload)?);
    }
    let mut hasher = Sha1::new();
    hasher.update(&out);
    out.extend_from_slice(&hasher.finalize());
    Ok(out)
}

/// `type`(3 bit) + `size`(4 + 7n bit) 的紧凑头。
fn write_object_header(out: &mut Vec<u8>, kind: crate::object::Kind, size: u64) {
    let type_code = match kind {
        crate::object::Kind::Commit => 1u8,
        crate::object::Kind::Tree => 2,
        crate::object::Kind::Blob => 3,
        crate::object::Kind::Tag => 4,
    };
    let mut byte = (type_code << 4) | ((size & 0x0f) as u8);
    let mut rest = size >> 4;
    while rest > 0 {
        out.push(byte | 0x80);
        byte = (rest & 0x7f) as u8;
        rest >>= 7;
    }
    out.push(byte);
}

/// 自己扫一遍 pack，拿到每个对象的起始偏移（以及「这是不是一个自包含的 pack」）。
///
/// 用 `zlib::inflate_prefix` 跳过压缩流：这样不必依赖 `.idx` 就能枚举对象，
/// 之后每个对象再用 T9 的 `PackFile::read_at` 解码（含 OFS_DELTA 链）。
fn scan_pack_offsets(pack: &[u8]) -> Result<Vec<u64>> {
    const HEADER: usize = 12;
    const TRAILER: usize = 20;
    if pack.len() < HEADER + TRAILER {
        return Err(Error::corrupt(
            "fetched pack",
            format!("only {} bytes long", pack.len()),
        ));
    }
    if !pack.starts_with(b"PACK") {
        return Err(Error::corrupt("fetched pack", "bad magic"));
    }
    let version = u32::from_be_bytes([pack[4], pack[5], pack[6], pack[7]]);
    if version != 2 && version != 3 {
        return Err(Error::corrupt(
            "fetched pack",
            format!("unsupported pack version {version}"),
        ));
    }
    let count = u32::from_be_bytes([pack[8], pack[9], pack[10], pack[11]]) as usize;
    let mut offsets = Vec::with_capacity(count);
    let mut pos = HEADER;
    for _ in 0..count {
        let start = pos;
        let byte = *pack
            .get(pos)
            .ok_or_else(|| Error::corrupt("fetched pack", "truncated object header"))?;
        pos += 1;
        let kind = (byte >> 4) & 0x07;
        let mut byte = byte;
        while byte & 0x80 != 0 {
            byte = *pack
                .get(pos)
                .ok_or_else(|| Error::corrupt("fetched pack", "truncated object header"))?;
            pos += 1;
        }
        match kind {
            1..=4 => {}
            6 => {
                let mut byte = *pack
                    .get(pos)
                    .ok_or_else(|| Error::corrupt("fetched pack", "truncated ofs-delta"))?;
                pos += 1;
                while byte & 0x80 != 0 {
                    byte = *pack
                        .get(pos)
                        .ok_or_else(|| Error::corrupt("fetched pack", "truncated ofs-delta"))?;
                    pos += 1;
                }
            }
            7 => {
                // REF_DELTA：base 靠 oid 反查。本仓库的 pack 永远不含 delta，
                // 因此这里明确报「不支持」，而不是猜（thin pack 需要本地已有 base）。
                return Err(Error::Unsupported(
                    "REF_DELTA in a received pack (thin pack): the base object must be \
                     looked up locally, which file:// v1 does not do",
                ));
            }
            other => {
                return Err(Error::corrupt(
                    "fetched pack",
                    format!("unknown object type {other}"),
                ))
            }
        }
        let rest = pack
            .get(pos..)
            .ok_or_else(|| Error::corrupt("fetched pack", "truncated object data"))?;
        let (_, consumed) = zlib::inflate_prefix(rest)?;
        pos += consumed;
        offsets.push(start as u64);
    }
    if pos + TRAILER != pack.len() {
        return Err(Error::corrupt(
            "fetched pack",
            format!(
                "{} bytes of trailing data after the last object",
                pack.len() - pos
            ),
        ));
    }
    let mut hasher = Sha1::new();
    hasher.update(&pack[..pos]);
    if hasher.finalize()[..] != pack[pos..] {
        return Err(Error::corrupt("fetched pack", "trailer sha1 mismatch"));
    }
    Ok(offsets)
}

/// 把收到的 pack 解成 loose 对象（复用 T9 的 `PackFile::read_at` + `PackSet`）。
///
/// 为了让 `PackFile::open` 有文件可读，pack 先落到 `objects/pack/` 下一个临时名字，
/// 装完对象再删掉；`PackSet` 在落盘**之前**打开，只用来判断「这个对象本地已经有了」。
fn install_pack(repo: &Repo, pack: &[u8]) -> Result<usize> {
    let existing = PackSet::open(repo)?;
    let offsets = scan_pack_offsets(pack)?;
    if offsets.is_empty() {
        return Ok(0);
    }
    let dir = repo.objects_dir().join("pack");
    fs::create_dir_all(&dir)?;
    let stem = format!("pack-mg{}-{}", std::process::id(), monotonic_seq());
    let pack_path = dir.join(format!("{stem}.pack"));
    fs::write(&pack_path, pack)?;

    let result = (|| -> Result<usize> {
        let pack_file = PackFile::open(&pack_path)?;
        let odb = Odb::new(repo);
        let mut written = 0usize;
        for offset in offsets {
            let (kind, payload) = pack_file.read_at(offset)?;
            let oid = Oid::hash_object(kind.as_str(), &payload);
            if odb.exists(oid) || existing.contains(oid) {
                continue;
            }
            odb.write(kind, &payload)?;
            written += 1;
        }
        Ok(written)
    })();

    let _ = fs::remove_file(&pack_path);
    result
}

fn monotonic_seq() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    SEQ.fetch_add(1, Ordering::SeqCst)
}

// ---------------------------------------------------------------- refspec

#[derive(Debug, Clone)]
struct Refspec {
    force: bool,
    src: String,
    dst: Option<String>,
}

fn parse_refspec(text: &str) -> Result<Refspec> {
    let (force, body) = match text.strip_prefix('+') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (src, dst) = match body.split_once(':') {
        Some((src, dst)) => (
            src.to_string(),
            if dst.is_empty() {
                None
            } else {
                Some(dst.to_string())
            },
        ),
        None => (body.to_string(), None),
    };
    if body.is_empty() {
        return Err(Error::Protocol("empty refspec".to_string()));
    }
    Ok(Refspec { force, src, dst })
}

/// 一个具体的「从远端哪个 ref 取到本地哪个 ref」。
#[derive(Debug, Clone)]
struct RefUpdate {
    source: String,
    oid: Oid,
    dest: Option<String>,
    force: bool,
}

/// 把 refspec 展开成具体的引用映射（支持 `*` 通配）。
fn expand_fetch_refspecs(adv: &RefAdvertisement, specs: &[Refspec]) -> Result<Vec<RefUpdate>> {
    // `refs/tags/v1^{}` 这样的剥离行只用于 tag 跟随，**永远不参与 refspec 匹配**
    // （否则会写出一个名字里带 `^{}` 的非法引用 —— 真实 git 也不会这么干）。
    let advertised: Vec<(String, Oid)> = adv
        .refs
        .iter()
        .filter(|(name, _)| name != CAPABILITIES_REF && !name.ends_with("^{}"))
        .cloned()
        .collect();
    let mut out = Vec::new();
    for spec in specs {
        // 通配 refspec **匹配不到任何引用不是错误**（真实 git：远端没有 tag 时
        // `+refs/tags/*:refs/tags/*` 只是什么都不取；`mg clone` 曾经因此报
        // `couldn't find remote ref refs/tags/*` 而拒绝克隆一个没有 tag 的仓库）。
        // 非通配的 `refs/heads/x` 匹配不到仍然是错误，由 `expand_one` 负责报。
        let matches = expand_one(&advertised, &spec.src)?;
        for (name, oid) in matches {
            let dest = match (&spec.dst, name.contains('*')) {
                (Some(dst), _) => Some(apply_wildcard_dest(dst, &spec.src, &name)),
                (None, _) => None,
            };
            out.push(RefUpdate {
                source: name,
                oid,
                dest,
                force: spec.force,
            });
        }
    }
    Ok(out)
}

/// 展开**单个** refspec 的源：通配符返回所有匹配（可能为空，不是错误）；
/// 非通配的源必须命中，否则 `couldn't find remote ref <src>`。
fn expand_one(advertised: &[(String, Oid)], src: &str) -> Result<Vec<(String, Oid)>> {
    if let Some(star) = src.find('*') {
        let prefix = &src[..star];
        let suffix = &src[star + 1..];
        return Ok(advertised
            .iter()
            .filter(|(name, _)| {
                name.len() >= prefix.len() + suffix.len()
                    && name.starts_with(prefix)
                    && name.ends_with(suffix)
            })
            .cloned()
            .collect());
    }
    let wanted = if src.starts_with("refs/") {
        vec![src.to_string()]
    } else {
        vec![
            format!("{HEADS_PREFIX}{src}"),
            format!("{TAGS_PREFIX}{src}"),
        ]
    };
    let found: Vec<(String, Oid)> = advertised
        .iter()
        .filter(|(name, _)| wanted.iter().any(|want| want == name))
        .cloned()
        .collect();
    if found.is_empty() {
        return Err(Error::Other(format!("couldn't find remote ref {src}")));
    }
    Ok(found)
}

fn apply_wildcard_dest(dst: &str, src: &str, matched: &str) -> String {
    match (src.find('*'), dst.find('*')) {
        (Some(src_star), Some(dst_star)) => {
            let middle = &matched[src_star..matched.len() - (src.len() - src_star - 1)];
            let mut out = String::from(&dst[..dst_star]);
            out.push_str(middle);
            out.push_str(&dst[dst_star + 1..]);
            out
        }
        _ => dst.to_string(),
    }
}

// ---------------------------------------------------------------- fetch

/// `file://`（或 `http://`）fetch：读广告 → 协商 → 收 pack → 写 loose 对象 → 更新本地引用。
pub fn fetch(repo: &Repo, url: &str, refspecs: &[String]) -> Result<FetchOutcome> {
    let conn = connect(url)?;
    let advertisement = conn.advertise(Service::UploadPack)?;
    let adv = negotiate::parse_advertisement(&advertisement)?;
    fetch_with(repo, conn.as_ref(), &adv, refspecs, url)
}

fn fetch_with(
    repo: &Repo,
    conn: &dyn Connection,
    adv: &RefAdvertisement,
    refspecs: &[String],
    url: &str,
) -> Result<FetchOutcome> {
    // 空远端（advertisement 只有 `capabilities^{}`）**不是**错误：真实 `git fetch` 在空远端上
    // exit 0 且一个引用都不写。这里不再特判，交给下面的 refspec 展开决定 ——
    // 通配 refspec 命中 0 个引用 → 下面的 `updates.is_empty()` 分支静默成功；
    // 非通配 refspec（如 `refs/heads/main`）查不到 → 仍然报 `couldn't find remote ref`（真实 git 同）。
    let specs: Vec<Refspec> = if refspecs.is_empty() {
        vec![parse_refspec(DEFAULT_FETCH_REFSPEC)?]
    } else {
        refspecs
            .iter()
            .map(|text| parse_refspec(text))
            .collect::<Result<Vec<_>>>()?
    };
    let updates = expand_fetch_refspecs(adv, &specs)?;

    let head = || match (adv.head_symref(), adv.get("HEAD")) {
        (Some(target), Some(oid)) if !oid.is_zero() => Some((target.to_string(), oid)),
        _ => None,
    };
    if updates.is_empty() {
        // 所有 refspec（含通配）都没命中任何引用 —— 包括**空远端**：真实 git 静默成功
        // （实测：`git fetch origin +refs/tags/*:refs/tags/*` 在没有 tag 的远端、以及
        // `git fetch origin` 在空远端上都是 exit 0），一个引用都不写；
        // 但真实 git **会把 FETCH_HEAD 截成空文件**（实测两种情形 size 都是 0），这里照做 ——
        // 既不发请求也不报错。
        write_fetch_head(repo, url, &[], adv.head_symref())?;
        return Ok(FetchOutcome {
            refs: Vec::new(),
            head: head(),
            objects_written: 0,
        });
    }
    let mut wants: Vec<Oid> = Vec::new();
    for update in &updates {
        if !wants.contains(&update.oid) {
            wants.push(update.oid);
        }
    }
    let haves: Vec<Oid> = RefStore::new(repo)
        .list()?
        .into_iter()
        .map(|(_, oid)| oid)
        .collect();

    let request = negotiate::build_fetch_request(
        &FetchRequest {
            wants: wants.clone(),
            haves,
            done: true,
        },
        &["side-band-64k", "no-progress"],
    );
    let response = conn.rpc(Service::UploadPack, &request)?;
    let pack = parse_upload_pack_response(&response)?;
    let objects_written = install_pack(repo, &pack)?;

    let refs = RefStore::new(repo);
    let head_symref = adv.head_symref().map(str::to_string);
    let mut written = Vec::new();
    let mut fetch_head = Vec::new();
    for update in &updates {
        let dest = match &update.dest {
            Some(dest) => dest.clone(),
            None => {
                fetch_head.push((None, update.source.clone(), update.oid));
                continue;
            }
        };
        if !dest.starts_with("refs/") {
            return Err(Error::Other(format!(
                "invalid refspec destination {dest:?}"
            )));
        }
        let before = refs.resolve(&dest).ok();
        if !update.force {
            if let Some(old) = before {
                if old != update.oid {
                    let fast_forward = crate::merge::merge_base(repo, update.oid, old)
                        .ok()
                        .flatten()
                        == Some(old);
                    if !fast_forward {
                        return Err(Error::Other(format!(
                            "refusing to update {dest}: not a fast-forward (use a + refspec)"
                        )));
                    }
                }
            }
        }
        refs.update(&dest, update.oid, Some(before))?;
        written.push((dest.clone(), update.oid));
        fetch_head.push((Some(dest), update.source.clone(), update.oid));
    }

    write_fetch_head(repo, url, &fetch_head, head_symref.as_deref())?;

    Ok(FetchOutcome {
        refs: written,
        head: head(),
        objects_written,
    })
}

/// 解析 `upload-pack` 响应：跳过 `NAK`/`ACK`，把 side-band 通道 1 拼成 pack。
fn parse_upload_pack_response(response: &[u8]) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(response);
    let mut pack = Vec::new();
    loop {
        match read_pkt(&mut cursor)? {
            Pkt::Data(data) => match data.first().copied() {
                Some(1) => pack.extend_from_slice(&data[1..]),
                Some(2) => {
                    // 进度：原样转发到 stderr（真实 git 也是这么做的）。
                    eprint!("{}", String::from_utf8_lossy(&data[1..]));
                }
                Some(3) => {
                    return Err(Error::Protocol(format!(
                        "remote error: {}",
                        String::from_utf8_lossy(&data[1..]).trim_end()
                    )))
                }
                _ => {
                    let text = String::from_utf8_lossy(&data);
                    if text.starts_with("NAK") || text.starts_with("ACK") {
                        continue;
                    }
                    return Err(Error::Protocol(format!(
                        "unexpected upload-pack response: {text:?}"
                    )));
                }
            },
            Pkt::Flush => break,
            Pkt::Delim | Pkt::ResponseEnd => break,
        }
    }
    if pack.is_empty() {
        return Err(Error::Protocol(
            "upload-pack returned no pack data on side-band channel 1".into(),
        ));
    }
    Ok(pack)
}

fn write_fetch_head(
    repo: &Repo,
    url: &str,
    entries: &[(Option<String>, String, Oid)],
    head_symref: Option<&str>,
) -> Result<()> {
    let mut body = String::new();
    for (dest, source, oid) in entries {
        let short = source
            .strip_prefix(HEADS_PREFIX)
            .map(|name| format!("branch '{name}'"))
            .or_else(|| {
                source
                    .strip_prefix(TAGS_PREFIX)
                    .map(|name| format!("tag '{name}'"))
            })
            .unwrap_or_else(|| source.clone());
        let is_head_branch = head_symref == Some(source.as_str());
        let for_merge = is_head_branch
            && dest
                .as_deref()
                .is_some_and(|dest| dest.starts_with(REMOTES_PREFIX));
        let line = if for_merge {
            format!("{}\t\t{short} of {url}\n", oid.to_hex())
        } else {
            format!("{}\t\tnot-for-merge\t{short} of {url}\n", oid.to_hex())
        };
        body.push_str(&line);
    }
    atomic_write_string(&repo.git_dir().join("FETCH_HEAD"), &body)
}

// ---------------------------------------------------------------- push

/// `file://`（或 `http://`）push：算闭包 → 生成 pack → 发 update + pack → 收 report-status。
pub fn push(repo: &Repo, url: &str, refspecs: &[String], force: bool) -> Result<()> {
    let conn = connect(url)?;
    let advertisement = conn.advertise(Service::ReceivePack)?;
    let adv = negotiate::parse_advertisement(&advertisement)?;
    push_with(repo, conn.as_ref(), &adv, refspecs, force, url)
}

fn push_with(
    repo: &Repo,
    conn: &dyn Connection,
    adv: &RefAdvertisement,
    refspecs: &[String],
    force: bool,
    url: &str,
) -> Result<()> {
    let refs = RefStore::new(repo);
    let specs: Vec<Refspec> = if refspecs.is_empty() {
        let branch = current_branch(repo)?.ok_or_else(|| {
            Error::Other(
                "you are not currently on a branch; give a refspec (mg push <remote> <ref>)".into(),
            )
        })?;
        vec![parse_refspec(&branch)?]
    } else {
        refspecs
            .iter()
            .map(|text| parse_refspec(text))
            .collect::<Result<Vec<_>>>()?
    };

    let mut cmds: Vec<PushCommand> = Vec::new();
    let mut summary: Vec<(String, String, Oid, Oid)> = Vec::new();
    for spec in &specs {
        if spec.src.is_empty() {
            return Err(Error::Unsupported(
                "ref deletion (`mg push remote :refs/heads/x`) is not supported in v1",
            ));
        }
        let src_full = if spec.src.starts_with("refs/") {
            spec.src.clone()
        } else {
            format!("{HEADS_PREFIX}{}", spec.src)
        };
        let new = refs
            .resolve(&spec.src)
            .map_err(|_| Error::Other(format!("src refspec {} does not match any", spec.src)))?;
        let dest = match &spec.dst {
            Some(dst) if dst.starts_with("refs/") => dst.clone(),
            Some(dst) => format!("{HEADS_PREFIX}{dst}"),
            None => src_full.clone(),
        };
        let old = adv.get(&dest).unwrap_or_else(Oid::zeros);
        if old == new {
            continue;
        }
        // `send-pack` 的本地判定：没有 --force 时非 fast-forward 一律拒绝，
        // 此时**连请求都不发**，远端一个字节都不会变。
        if !force && !old.is_zero() {
            let fast_forward = crate::merge::merge_base(repo, new, old).ok().flatten() == Some(old);
            if !fast_forward {
                return Err(Error::Other(format!(
                    "failed to push some refs: {dest} is not a fast-forward \
                     (the remote ref moved; fetch and merge, or use --force)"
                )));
            }
        }
        let short_src = spec.src.clone();
        summary.push((short_src, dest.clone(), old, new));
        cmds.push(PushCommand {
            old,
            new,
            name: dest,
        });
    }

    if cmds.is_empty() {
        println!("Everything up-to-date");
        return Ok(());
    }

    // 发送对象的集合 = 本地可达(new) - 本地可达(远端已有的 oid)。
    let mut send: BTreeSet<Oid> = BTreeSet::new();
    let known_remote: BTreeSet<Oid> = adv
        .refs
        .iter()
        .map(|(_, oid)| *oid)
        .filter(|oid| !oid.is_zero())
        .collect();
    let known_remote: Vec<Oid> = known_remote.into_iter().collect();
    let remote_closure = reachable(repo, &known_remote, true)?;
    for (_, _, _, new) in &summary {
        for oid in reachable(repo, &[*new], false)? {
            if !remote_closure.contains(&oid) {
                send.insert(oid);
            }
        }
    }
    let pack = build_pack(repo, &send)?;

    let mut request = negotiate::build_push_update(&cmds, &["report-status"])?;
    request.extend_from_slice(&pack);
    let response = conn.rpc(Service::ReceivePack, &request)?;
    let status = negotiate::parse_report_status(&response)?;

    let mut failures = Vec::new();
    for (name, status) in &status {
        if name == "unpack" {
            if status != "ok" {
                failures.push(format!("unpack failed: {status}"));
            }
        } else if status != "ok" {
            failures.push(format!("{name}: {}", status.trim_start_matches("ng ")));
        }
    }
    if !failures.is_empty() {
        return Err(Error::Other(format!(
            "failed to push some refs to '{url}'\n{}",
            failures.join("\n")
        )));
    }

    println!("To {url}");
    for (src, dest, old, new) in &summary {
        let old_hex = if old.is_zero() {
            "[new branch]".to_string()
        } else {
            old.to_hex()[..7].to_string()
        };
        let new_hex = &new.to_hex()[..7];
        println!(
            "   {}..{}  {} -> {}",
            old_hex,
            new_hex,
            short_name(src),
            short_name(dest)
        );
    }
    Ok(())
}

fn short_name(name: &str) -> &str {
    for prefix in [HEADS_PREFIX, TAGS_PREFIX, REMOTES_PREFIX] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest;
        }
    }
    name
}

fn current_branch(repo: &Repo) -> Result<Option<String>> {
    match RefStore::new(repo).read_head()? {
        Head::Attached(name) => Ok(name
            .strip_prefix(HEADS_PREFIX)
            .map(str::to_string)
            .or(Some(name))),
        Head::Detached(_) => Ok(None),
    }
}

// ---------------------------------------------------------------- clone

/// 空远端时真实 git 打的那一行 warning（逐字；测试从真实 git 的 stderr 取真值比对）。
pub const EMPTY_REMOTE_WARNING: &str = "warning: You appear to have cloned an empty repository.";

/// `mg clone <url> <dir>`：建新仓库 → fetch → 检出 HEAD。
///
/// 空远端（advertisement 里只有 `capabilities^{}`）**不是**错误：真实 git 2.55.0
/// （默认 protocol v2）对其 exit 0、打一条 warning、留下一个只含 `.git` 的克隆
/// （无工作区文件、无引用、无 `FETCH_HEAD`）。V13 的 Q1 裁决 = 跟随真实 git。
pub fn clone_into(url: &str, dir: &Path) -> Result<Repo> {
    let conn = connect(url)?;
    let advertisement = conn.advertise(Service::UploadPack)?;
    let adv = negotiate::parse_advertisement(&advertisement)?;
    if adv.is_empty_repository() {
        return clone_empty_remote(url, dir, &adv);
    }

    let head_oid = adv.get("HEAD").filter(|oid| !oid.is_zero());
    // 分支名：优先 `symref=HEAD:...`；没有 symref（远端 detached）时，
    // 像真实 git 一样「猜」——HEAD 的 oid 与哪个 `refs/heads/*` 相同就用哪个
    // （实测：远端 detached 但分支仍在时，`git clone` 依然会建出那个本地分支）。
    let branch = match adv
        .head_symref()
        .and_then(|target| target.strip_prefix(HEADS_PREFIX))
    {
        Some(branch) => branch.to_string(),
        None => head_oid
            .and_then(|oid| {
                adv.refs
                    .iter()
                    .find(|(name, candidate)| name.starts_with(HEADS_PREFIX) && *candidate == oid)
                    .map(|(name, _)| name.trim_start_matches(HEADS_PREFIX).to_string())
            })
            .unwrap_or_else(|| crate::repo::DEFAULT_INITIAL_BRANCH.to_string()),
    };

    // 真实 git 在「远端 HEAD 指向一个不存在的引用」时会照常 clone（不检出），
    // 只打一条 warning —— 实测（远端 HEAD -> refs/heads/master 但只有 refs/heads/main）。
    if let Some(target) = adv.head_symref() {
        if adv.get(target).is_none() {
            eprintln!("warning: remote HEAD refers to nonexistent ref, unable to checkout");
        }
    }

    let created = check_clone_destination(dir)?;

    let result = (|| -> Result<Repo> {
        let repo = Repo::init(dir, &branch)?;
        let refspecs = vec![
            DEFAULT_FETCH_REFSPEC.to_string(),
            format!("+{TAGS_PREFIX}*:{TAGS_PREFIX}*"),
        ];
        fetch_with(&repo, conn.as_ref(), &adv, &refspecs, url)?;

        // `refs/remotes/<remote>/HEAD`：真实 git clone 会写这个符号引用，
        // 否则 `git branch -r` 看不到 `origin/HEAD -> origin/main`。
        let remote_head = format!("{REMOTES_PREFIX}{DEFAULT_REMOTE}/HEAD");
        let remote_branch = format!("{REMOTES_PREFIX}{DEFAULT_REMOTE}/{branch}");
        if RefStore::new(&repo).exists(&remote_branch) {
            write_symref(&repo, &remote_head, &remote_branch)?;
        }

        if let Some(oid) = head_oid {
            let local = format!("{HEADS_PREFIX}{branch}");
            let before = RefStore::new(&repo).resolve(&local).ok();
            RefStore::new(&repo).update(&local, oid, Some(before))?;
            checkout_to(&repo, oid, true, HeadTarget::Attached(&branch))?;
        }
        write_remote_config(&repo, DEFAULT_REMOTE, url, &branch)?;
        Ok(repo)
    })();

    if result.is_err() {
        remove_half_made_clone(dir, created);
    }
    result
}

/// 空远端的 clone：与真实 git 一样只建仓库（含 remote/branch 配置与 HEAD 符号引用），
/// 跳过 fetch 与检出，不写任何引用、不建 `FETCH_HEAD`、不产生工作区文件。
///
/// 分支名取自广告里的 `symref=HEAD:...`：本进程内 `file://` 服务端即使远端为空也会发这个
/// 能力（`HEAD` 是 unborn 的符号引用），语义上等价于真实 git 客户端在 v2 下的
/// `ls-refs=unborn`（实测 `git clone file://<空裸库>` 正是靠它建出 `refs/heads/main`）。
/// 广告里没有 symref（例如空远端的 HTTP 广告）时退回本地默认初始分支。
fn clone_empty_remote(url: &str, dir: &Path, adv: &RefAdvertisement) -> Result<Repo> {
    let branch = adv
        .head_symref()
        .and_then(|target| target.strip_prefix(HEADS_PREFIX))
        .unwrap_or(crate::repo::DEFAULT_INITIAL_BRANCH)
        .to_string();
    let created = check_clone_destination(dir)?;
    let result = (|| -> Result<Repo> {
        let repo = Repo::init(dir, &branch)?;
        write_remote_config(&repo, DEFAULT_REMOTE, url, &branch)?;
        eprintln!("{EMPTY_REMOTE_WARNING}");
        Ok(repo)
    })();
    if result.is_err() {
        remove_half_made_clone(dir, created);
    }
    result
}

/// 校验 clone 的目标目录（三种判据与真实 git 相同）。
/// 返回 `true` 表示目录不存在、需要创建。
fn check_clone_destination(dir: &Path) -> Result<bool> {
    if !dir.exists() {
        return Ok(true);
    }
    if !dir.is_dir() {
        return Err(Error::Other(format!(
            "destination path '{}' exists and is not a directory",
            dir.display()
        )));
    }
    if fs::read_dir(dir)?.next().is_some() {
        return Err(Error::Other(format!(
            "destination path '{}' already exists and is not an empty directory",
            dir.display()
        )));
    }
    Ok(false)
}

/// 失败不留半成品目录（真实 git 也只在完全成功后才留下克隆）。
fn remove_half_made_clone(dir: &Path, created: bool) {
    let _ = if created {
        fs::remove_dir_all(dir)
    } else {
        fs::remove_dir_all(dir.join(".git"))
    };
}

/// 写 remote/branch 配置。`Repo`/`Config` 只有读接口，而本任务的写作用域不允许改
/// `src/repo.rs`，也不允许新增文件，因此配置写入放在这里（原子：临时文件 + rename）。
fn write_remote_config(repo: &Repo, remote: &str, url: &str, branch: &str) -> Result<()> {
    let path = repo.git_dir().join("config");
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!(
        "[remote \"{remote}\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/{remote}/*\n",
        url.replace('\\', "\\\\").replace('\n', " ")
    ));
    text.push_str(&format!(
        "[branch \"{branch}\"]\n\tremote = {remote}\n\tmerge = refs/heads/{branch}\n"
    ));
    write_branch_upstream_into(&path, &text)
}

/// `-u/--set-upstream`：追加 `branch.<name>.remote` / `branch.<name>.merge`。
pub(crate) fn set_branch_upstream(
    repo: &Repo,
    branch: &str,
    remote: &str,
    merge_ref: &str,
) -> Result<()> {
    let path = repo.git_dir().join("config");
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!(
        "[branch \"{branch}\"]\n\tremote = {remote}\n\tmerge = {merge_ref}\n"
    ));
    write_branch_upstream_into(&path, &text)
}

/// 写一个符号引用文件（`ref:<space><target>\n`），与真实 git 的落盘格式一致。
fn write_symref(repo: &Repo, name: &str, target: &str) -> Result<()> {
    let path = repo.git_dir().join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write_string(&path, &format!("ref: {target}\n"))
}

fn write_branch_upstream_into(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_file_name("config.mg-tmp");
    fs::write(&tmp, text)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err.into())
        }
    }
}

fn atomic_write_string(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_file_name(format!(
        "{}.mg-tmp",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "mg".to_string())
    ));
    fs::write(&tmp, text)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            Err(err.into())
        }
    }
}

#[cfg(test)]
mod tests {
    //! 真值全部来自**真实 git 进程**（`git fsck` / `git log` / `git status` /
    //! `git show-ref` / `git rev-parse`）或字节层面的手工断言。
    //!
    //! 环境缺失（`git` 不在、`mg` 二进制没构建）时**直接 panic**，
    //! 不允许「跳过即通过」。

    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    use super::*;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    /// 同 `git`，但**允许**非 0 退出（真值对拍常用：真实 git 的「没有任何引用」就是 exit 1）。
    fn git_raw(dir: &Path, args: &[&str]) -> Output {
        assert!(git_available(), "this test needs a real `git` on PATH");
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "A U Thor")
            .env("GIT_AUTHOR_EMAIL", "a@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0800")
            .env("GIT_COMMITTER_NAME", "A U Thor")
            .env("GIT_COMMITTER_EMAIL", "a@example.com")
            .env("GIT_COMMITTER_DATE", "1700000000 +0800")
            .output()
            .expect("failed to spawn git")
    }

    fn git(dir: &Path, args: &[&str]) -> Output {
        let out = git_raw(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn git_text(dir: &Path, args: &[&str]) -> String {
        String::from_utf8(git(dir, args).stdout)
            .expect("git stdout is UTF-8")
            .trim()
            .to_string()
    }

    fn write(dir: &Path, rel: &str, contents: &[u8]) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn commit(dir: &Path, message: &str) {
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", message]);
    }

    /// `mg` 二进制：优先用 cargo 给测试注入的路径，其次 `target/debug/mg`。
    fn mg_bin() -> PathBuf {
        if let Ok(path) = std::env::var("CARGO_BIN_EXE_mg") {
            return PathBuf::from(path);
        }
        let target = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));
        let candidate = target.join("debug").join("mg");
        assert!(
            candidate.is_file(),
            "mg binary not found at {}; build it first (`cargo build`)",
            candidate.display()
        );
        candidate
    }

    fn mg(dir: &Path, args: &[&str]) -> Output {
        let out = Command::new(mg_bin())
            .args(args)
            .current_dir(dir)
            .output()
            .expect("failed to spawn mg");
        assert!(
            out.status.success(),
            "mg {args:?} failed ({}):\nstdout: {}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    /// 远端有哪些 tag。
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum OriginTags {
        None,
        Light,
        Annot,
        Both,
    }

    /// 一个「像样」的真实 git 仓库：子目录、两个提交、一个分支、tag 按 `tags` 给，
    /// `detach` 为真时把远端 HEAD 变成 detached。
    fn make_origin(label: &str, tags: OriginTags, detach: bool) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join(label);
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", "."]);
        git(&work, &["config", "user.name", "A U Thor"]);
        git(&work, &["config", "user.email", "a@example.com"]);
        // 内容里带上 label：不同 fixture 的对象 id 必须不同，否则「远端已有」会被误判。
        write(&work, "a.txt", format!("one {label}\n").as_bytes());
        write(&work, "sub/deep/b.txt", b"two\n");
        write(&work, "exec.sh", b"#!/bin/sh\necho hi\n");
        std::fs::set_permissions(
            work.join("exec.sh"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        commit(&work, "one");
        write(&work, "a.txt", format!("one {label}\ntwo\n").as_bytes());
        commit(&work, "two");
        git(&work, &["branch", "feature"]);
        match tags {
            OriginTags::None => {}
            OriginTags::Light => {
                git(&work, &["tag", "light"]);
            }
            OriginTags::Annot => {
                git(&work, &["tag", "-a", "v1", "-m", "tag message"]);
            }
            OriginTags::Both => {
                git(&work, &["tag", "-a", "v1", "-m", "tag message"]);
                git(&work, &["tag", "light"]);
            }
        }
        if detach {
            git(&work, &["checkout", "-q", "--detach"]);
        }
        (dir, work)
    }

    /// 默认夹具（两个 tag、attached HEAD）—— 上一轮 clone 测试的形态。
    fn make_src(label: &str) -> (tempfile::TempDir, PathBuf) {
        make_origin(label, OriginTags::Both, false)
    }

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    fn empty_bare(label: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join(label);
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare", "-b", "main", "."]);
        (dir, bare)
    }

    // ---------------------------------------------------------- 广告字节

    #[test]
    fn advertisement_bytes_have_the_v0_pkt_line_shape() {
        let (_dir, src) = make_src("adv");
        let repo = Repo::discover(&src).unwrap();
        let raw = LocalRemote { repo: &repo }
            .advertise(Service::UploadPack)
            .unwrap();

        // 逐帧读回来：首帧是 `<HEAD oid> HEAD\0<caps>`，末帧是 flush。
        let mut cursor = std::io::Cursor::new(raw.as_slice());
        let mut frames = Vec::new();
        let mut rebuilt = Vec::new();
        loop {
            match crate::transport::pktline::read_pkt(&mut cursor).unwrap() {
                Pkt::Data(data) => {
                    rebuilt.extend_from_slice(&crate::transport::pktline::encode_pkt(&data));
                    frames.push(data);
                }
                Pkt::Flush => {
                    rebuilt.extend_from_slice(b"0000");
                    break;
                }
                other => panic!("unexpected frame {other:?}"),
            }
        }
        assert_eq!(rebuilt, raw, "advertisement must round-trip byte for byte");

        let head_hex = git_text(&src, &["rev-parse", "HEAD"]);
        let first = String::from_utf8(frames[0].clone()).unwrap();
        assert!(
            first.starts_with(&format!("{head_hex} HEAD\0")),
            "{first:?}"
        );
        assert!(first.contains("side-band-64k"), "{first:?}");
        assert!(first.contains("symref=HEAD:refs/heads/main"), "{first:?}");
        assert!(first.ends_with('\n'));
        assert!(
            frames
                .iter()
                .any(|frame| frame.starts_with(b"refs/heads/main".as_slice())
                    || frame.windows(15).any(|w| w == b"refs/heads/main")),
            "the advertisement must list refs/heads/main"
        );

        let parsed = negotiate::parse_advertisement(&raw).unwrap();
        assert_eq!(
            parsed.get("HEAD").map(|o| o.to_hex()),
            Some(head_hex.clone())
        );
        assert!(!parsed.is_empty_repository());
        assert_eq!(
            parsed.get("refs/tags/v1").map(|o| o.to_hex()),
            Some(git_text(&src, &["rev-parse", "v1"]))
        );

        let receive = LocalRemote { repo: &repo }
            .advertise(Service::ReceivePack)
            .unwrap();
        let parsed = negotiate::parse_advertisement(&receive).unwrap();
        assert!(parsed.has_capability("report-status"));
        assert!(parsed.has_capability("ofs-delta"));
    }

    // ---------------------------------------------------------- pack 字节

    #[test]
    fn pack_layout_is_self_contained_and_installable() {
        let (_dir, src) = make_src("pack");
        let repo = Repo::discover(&src).unwrap();
        let head = RefStore::new(&repo).resolve("HEAD").unwrap();
        let closure = reachable(&repo, &[head], false).unwrap();
        assert!(closure.len() >= 5, "commits + trees + blobs: {closure:?}");

        let pack = build_pack(&repo, &closure).unwrap();
        // 头部：字节级断言。
        assert_eq!(&pack[..4], b"PACK");
        assert_eq!(
            u32::from_be_bytes([pack[4], pack[5], pack[6], pack[7]]),
            2,
            "pack version"
        );
        assert_eq!(
            u32::from_be_bytes([pack[8], pack[9], pack[10], pack[11]]) as usize,
            closure.len(),
            "object count in the header"
        );
        // 尾部：前 12+n 字节的 sha1。
        let mut hasher = Sha1::new();
        hasher.update(&pack[..pack.len() - 20]);
        assert_eq!(hasher.finalize()[..], pack[pack.len() - 20..]);

        // 自己扫出来的偏移：升序、数量与闭包一致，且每个偏移都能被 T9 读回来
        // （read_at 会解开 delta 链并校验解压长度）。
        let offsets = scan_pack_offsets(&pack).unwrap();
        assert_eq!(offsets.len(), closure.len());
        assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
        {
            let scan_dir = tempfile::tempdir().unwrap();
            let scan_path = scan_dir.path().join("scan.pack");
            std::fs::write(&scan_path, &pack).unwrap();
            let pack_file = PackFile::open(&scan_path).unwrap();
            assert_eq!(pack_file.object_count() as usize, closure.len());
            let mut seen = BTreeSet::new();
            for offset in &offsets {
                let (kind, payload) = pack_file.read_at(*offset).unwrap();
                seen.insert(Oid::hash_object(kind.as_str(), &payload));
            }
            assert_eq!(seen, closure, "the pack must hold exactly the closure");
        }

        // 装进一个全新仓库：每个对象都要能用真实 git 读出来。
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        Repo::init(&dst, "main").unwrap();
        let dst_repo = Repo::discover(&dst).unwrap();
        let written = install_pack(&dst_repo, &pack).unwrap();
        assert_eq!(written, closure.len());
        for oid in &closure {
            let hex = oid.to_hex();
            assert_eq!(
                git_text(&dst, &["cat-file", "-t", &hex]),
                match Odb::new(&repo).read(*oid).unwrap().0 {
                    crate::object::Kind::Commit => "commit",
                    crate::object::Kind::Tree => "tree",
                    crate::object::Kind::Blob => "blob",
                    crate::object::Kind::Tag => "tag",
                }
            );
        }
        // 幂等：再装一次不会有新对象。
        assert_eq!(install_pack(&dst_repo, &pack).unwrap(), 0);

        // 反例：不是 pack / 截断 / 尾部被改。
        assert!(scan_pack_offsets(b"NOTAPACK").is_err());
        assert!(scan_pack_offsets(&pack[..pack.len() - 1]).is_err());
        let mut tampered = pack.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        assert!(scan_pack_offsets(&tampered).is_err());
    }

    #[test]
    fn ref_delta_packs_are_reported_as_unsupported() {
        // 手工造一个「含 REF_DELTA 的 pack」：type=7、size=0、20 字节 base oid、zlib 的 delta。
        let mut pack = Vec::new();
        pack.extend_from_slice(b"PACK");
        pack.extend_from_slice(&2u32.to_be_bytes());
        pack.extend_from_slice(&1u32.to_be_bytes());
        pack.push(0x70); // type 7 (REF_DELTA), size 0
        pack.extend_from_slice(&[0x11; 20]);
        pack.extend_from_slice(&crate::zlib::deflate(&[0u8; 4]).unwrap());
        let mut hasher = Sha1::new();
        hasher.update(&pack);
        pack.extend_from_slice(&hasher.finalize());
        match scan_pack_offsets(&pack) {
            Err(Error::Unsupported(_)) => {}
            other => panic!("a REF_DELTA pack must be reported as Unsupported: {other:?}"),
        }
    }

    // ---------------------------------------------------------- clone

    #[test]
    fn clone_matches_real_git() {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let (_src_dir, src) = make_src("clone-src");
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");

        let repo = clone_into(&file_url(&src), &dst).unwrap();
        assert_eq!(
            repo.workdir(),
            Some(fs::canonicalize(&dst).unwrap().as_path())
        );

        // git fsck：无 error（退出码 0）。
        let fsck = git(&dst, &["fsck", "--no-progress"]);
        assert!(fsck.status.success());
        // git log：与源仓库完全一致。
        assert_eq!(
            git_text(&dst, &["log", "--oneline"]),
            git_text(&src, &["log", "--oneline"])
        );
        // git status：干净。
        assert_eq!(git_text(&dst, &["status", "--porcelain"]), "");
        // 工作区内容与 mode 都对。
        assert_eq!(
            std::fs::read(dst.join("a.txt")).unwrap(),
            format!("one {}\ntwo\n", "clone-src").as_bytes()
        );
        assert_eq!(std::fs::read(dst.join("sub/deep/b.txt")).unwrap(), b"two\n");
        let mode = std::fs::metadata(dst.join("exec.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755, "the executable bit must survive the clone");

        // 远端引用与 tag 都对（真实 git 读得到）。
        assert_eq!(
            git_text(&dst, &["rev-parse", "refs/remotes/origin/main"]),
            git_text(&src, &["rev-parse", "HEAD"])
        );
        assert_eq!(
            git_text(&dst, &["rev-parse", "refs/remotes/origin/feature"]),
            git_text(&src, &["rev-parse", "refs/heads/feature"])
        );
        assert_eq!(
            git_text(&dst, &["rev-parse", "refs/tags/v1"]),
            git_text(&src, &["rev-parse", "refs/tags/v1"])
        );
        // 配置：remote.origin.url / fetch、branch.main.merge 都能被真实 git 读到。
        assert_eq!(
            git_text(&dst, &["config", "--get", "remote.origin.url"]),
            file_url(&src)
        );
        assert_eq!(
            git_text(&dst, &["config", "--get", "branch.main.merge"]),
            "refs/heads/main"
        );
        assert_eq!(git_text(&dst, &["config", "--get", "core.bare"]), "false");
        // HEAD 指向远端 HEAD 的分支。
        assert_eq!(
            std::fs::read_to_string(dst.join(".git/HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
        // `refs/remotes/origin/HEAD` 是真实 git 认得的符号引用。
        assert_eq!(
            std::fs::read_to_string(dst.join(".git/refs/remotes/origin/HEAD")).unwrap(),
            "ref: refs/remotes/origin/main\n"
        );
        assert_eq!(
            git_text(&dst, &["rev-parse", "refs/remotes/origin/HEAD"]),
            git_text(&src, &["rev-parse", "HEAD"])
        );
        // 与真实 `git clone file://...` 的产物逐项对拍（引用集合与 `-r` 列表）。
        let real = dir.path().join("real");
        git(
            dir.path(),
            &["clone", "-q", &file_url(&src), real.to_str().unwrap()],
        );
        assert_eq!(
            git_text(&dst, &["branch", "-r"]),
            git_text(&real, &["branch", "-r"]),
            "remote-tracking refs must match a real git clone"
        );
        let mut ours: Vec<String> = git_text(&dst, &["show-ref"])
            .lines()
            .map(str::to_string)
            .collect();
        let mut theirs: Vec<String> = git_text(&real, &["show-ref"])
            .lines()
            .map(str::to_string)
            .collect();
        ours.sort();
        theirs.sort();
        assert_eq!(ours, theirs, "show-ref must match a real git clone");
    }

    #[test]
    fn clone_failures_leave_no_half_made_directory() {
        let (root, src) = make_src("clone-err");

        // 1) URL 不存在。
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("dst-missing");
        let err = clone_into(&file_url(&root.path().join("nope")), &missing);
        assert!(err.is_err());
        assert!(
            !missing.exists(),
            "no half-made directory may be left behind"
        );

        // 2) 目录存在但不是仓库。
        let not_repo = root.path().join("notrepo");
        std::fs::create_dir_all(&not_repo).unwrap();
        let dst = dir.path().join("dst-notrepo");
        assert!(clone_into(&file_url(&not_repo), &dst).is_err());
        assert!(!dst.exists());

        // 3) 空远端**不再是**反例：Q1 裁决 = 跟随真实 git（exit 0，见
        //    `clone_of_an_empty_remote_matches_real_git`）。
        //    但目标目录已存在/非空仍然是反例（空远端也不例外）。
        let (_empty_dir, empty) = empty_bare("empty.git");
        let dst = dir.path().join("dst-empty-occupied");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("keep.txt"), b"keep\n").unwrap();
        assert!(clone_into(&file_url(&empty), &dst).is_err());
        assert_eq!(std::fs::read(dst.join("keep.txt")).unwrap(), b"keep\n");

        // 4) 读不到的目录（权限）：报错且不 panic、不留目录。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let outer = root.path().join("outer");
            std::fs::create_dir_all(&outer).unwrap();
            std::fs::set_permissions(&outer, std::fs::Permissions::from_mode(0o000)).unwrap();
            let dst = dir.path().join("dst-noperm");
            let result = clone_into(&file_url(&outer.join("inner")), &dst);
            std::fs::set_permissions(&outer, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(result.is_err(), "an unreadable remote must be an error");
            assert!(!dst.exists());
        }

        // 5) 目标目录非空。
        let dst = dir.path().join("dst-occupied");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("keep.txt"), b"keep\n").unwrap();
        assert!(clone_into(&file_url(&src), &dst).is_err());
        assert_eq!(std::fs::read(dst.join("keep.txt")).unwrap(), b"keep\n");

        // 6) 不支持的 scheme。
        let dst = dir.path().join("dst-https");
        assert!(matches!(
            clone_into("https://example.com/x.git", &dst),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            clone_into("git://example.com/x.git", &dst),
            Err(Error::Unsupported(_))
        ));
        assert!(!dst.exists());
        assert!(src.is_dir());
    }

    /// D2 + 任务书 §1 的夹具矩阵：**mg clone 必须与真实 `git clone file://…` 逐项一致**。
    ///
    /// 判据（逐条 assert）：`git show-ref`（排序后，含 `refs/tags/*` 的**有无**）、
    /// `git branch -a`、`.git/HEAD`、`.git/refs/remotes/origin/HEAD`、`git log --oneline`、
    /// `git status --porcelain`（必须为空）、`git fsck --no-progress`（必须无 error）。
    ///
    /// 覆盖：无 tag（D2 核心）/ 只有轻量 tag / 只有 annotated tag / 两者都有 /
    /// 两者都有但 HEAD detached / 无 tag 且 HEAD detached。
    #[test]
    fn clone_matches_real_git_across_fixtures() {
        let cases = [
            ("no-tags", OriginTags::None, false),
            ("light-only", OriginTags::Light, false),
            ("annot-only", OriginTags::Annot, false),
            ("both-tags", OriginTags::Both, false),
            ("detached", OriginTags::Both, true),
            ("detached-no-tags", OriginTags::None, true),
        ];
        for (label, tags, detach) in cases {
            let (_origin_dir, src) = make_origin(label, tags, detach);
            let dir = tempfile::tempdir().unwrap();
            let dst = dir.path().join("mg-clone");
            clone_into(&file_url(&src), &dst)
                .unwrap_or_else(|err| panic!("[{label}] mg clone failed: {err}"));

            let real = dir.path().join("git-clone");
            git(
                dir.path(),
                &["clone", "-q", &file_url(&src), real.to_str().unwrap()],
            );

            let read = |path: &Path| std::fs::read_to_string(path).ok();
            for (what, ours, theirs) in [
                (
                    "show-ref",
                    git_text(&dst, &["show-ref"]),
                    git_text(&real, &["show-ref"]),
                ),
                (
                    "branch -a",
                    git_text(&dst, &["branch", "-a"]),
                    git_text(&real, &["branch", "-a"]),
                ),
                (
                    "log --oneline",
                    git_text(&dst, &["log", "--oneline"]),
                    git_text(&real, &["log", "--oneline"]),
                ),
                (
                    "status --porcelain",
                    git_text(&dst, &["status", "--porcelain"]),
                    git_text(&real, &["status", "--porcelain"]),
                ),
                (
                    "HEAD",
                    read(&dst.join(".git/HEAD")).unwrap_or_default(),
                    read(&real.join(".git/HEAD")).unwrap_or_default(),
                ),
                (
                    "refs/remotes/origin/HEAD",
                    read(&dst.join(".git/refs/remotes/origin/HEAD")).unwrap_or_default(),
                    read(&real.join(".git/refs/remotes/origin/HEAD")).unwrap_or_default(),
                ),
            ] {
                let mut ours_sorted: Vec<&str> = ours.lines().collect();
                let mut theirs_sorted: Vec<&str> = theirs.lines().collect();
                ours_sorted.sort();
                theirs_sorted.sort();
                assert_eq!(
                    ours_sorted, theirs_sorted,
                    "[{label}] {what} must match a real git clone"
                );
            }
            assert_eq!(
                git_text(&dst, &["status", "--porcelain"]),
                "",
                "[{label}] the clone must be clean"
            );
            git(&dst, &["fsck", "--no-progress"]);
            // 远端确实没有 tag 的夹具：不能凭空多出 refs/tags。
            if tags == OriginTags::None {
                assert_eq!(
                    git_text(&dst, &["for-each-ref", "refs/tags/"]),
                    "",
                    "[{label}] a tagless origin must yield a tagless clone"
                );
            }
        }
    }

    fn list_dir(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Q1 裁决（跟随真实 git）：空远端（空裸库 / 空非裸库）上的 `mg clone` 必须与
    /// 真实 `git clone file://…` 逐字段一致 —— exit code、stdout/stderr 逐字节、
    /// 目标目录内容、`show-ref`、`symbolic-ref HEAD`、`status --porcelain`、
    /// `HEAD`/`FETCH_HEAD`/`refs/remotes/origin/HEAD` 的有无、配置键。
    #[test]
    fn clone_of_an_empty_remote_matches_real_git() {
        for (label, bare) in [("empty-bare", true), ("empty-plain", false)] {
            let root = tempfile::tempdir().unwrap();
            let src = root.path().join(label);
            std::fs::create_dir_all(&src).unwrap();
            git(
                &src,
                if bare {
                    &["init", "-q", "--bare", "-b", "main", "."]
                } else {
                    &["init", "-q", "-b", "main", "."]
                },
            );
            assert_eq!(
                git_text(&src, &["symbolic-ref", "HEAD"]),
                "refs/heads/main",
                "[{label}] fixture: HEAD must point at an unborn branch"
            );
            assert!(
                !git_raw(&src, &["show-ref"]).status.success(),
                "[{label}] fixture: the remote must advertise no refs at all"
            );

            // 两个独立 cwd + **同一个相对目标名**，这样 stderr 可以逐字节比对
            // （真实 git 打印的是命令行里给出的那个路径）。
            let mine_dir = root.path().join("mine");
            let real_dir = root.path().join("theirs");
            std::fs::create_dir_all(&mine_dir).unwrap();
            std::fs::create_dir_all(&real_dir).unwrap();
            let mine = Command::new(mg_bin())
                .args(["clone", &file_url(&src), "clone"])
                .current_dir(&mine_dir)
                .output()
                .expect("failed to spawn mg");
            let real = git_raw(&real_dir, &["clone", &file_url(&src), "clone"]);
            let ours = mine_dir.join("clone");
            let theirs = real_dir.join("clone");

            // 1) exit code、stdout、stderr。
            assert_eq!(
                mine.status.code(),
                Some(0),
                "[{label}] an empty remote must clone successfully (stderr: {})",
                String::from_utf8_lossy(&mine.stderr)
            );
            assert_eq!(
                mine.status.code(),
                real.status.code(),
                "[{label}] exit codes must match a real git clone"
            );
            assert_eq!(
                String::from_utf8_lossy(&mine.stdout),
                String::from_utf8_lossy(&real.stdout),
                "[{label}] stdout must match a real git clone"
            );
            assert_eq!(
                String::from_utf8_lossy(&mine.stderr),
                String::from_utf8_lossy(&real.stderr),
                "[{label}] stderr must match a real git clone byte for byte"
            );
            // warning 文案的真值直接取自真实 git 的 stderr（零硬编码）。
            let real_warning = String::from_utf8_lossy(&real.stderr)
                .lines()
                .find(|line| line.starts_with("warning:"))
                .unwrap_or_else(|| {
                    panic!(
                        "[{label}] real git printed no warning: {}",
                        String::from_utf8_lossy(&real.stderr)
                    )
                })
                .to_string();
            assert_eq!(
                EMPTY_REMOTE_WARNING, real_warning,
                "[{label}] the mg warning must be byte-identical to real git's"
            );

            // 2) 目标目录内容：顶层只有 `.git`，没有工作区文件、没有多余的引用文件。
            assert_eq!(
                list_dir(&ours),
                vec![".git".to_string()],
                "[{label}] an empty clone must leave only .git"
            );
            assert_eq!(
                list_dir(&ours),
                list_dir(&theirs),
                "[{label}] the worktree must contain exactly what a real git clone leaves"
            );
            for rel in [".git/HEAD", ".git/config"] {
                assert_eq!(
                    std::fs::read_to_string(ours.join(rel)).unwrap(),
                    std::fs::read_to_string(theirs.join(rel)).unwrap(),
                    "[{label}] {rel} must match a real git clone"
                );
            }
            for rel in [
                ".git/FETCH_HEAD",
                ".git/packed-refs",
                ".git/refs/remotes/origin/HEAD",
            ] {
                assert!(
                    !ours.join(rel).exists(),
                    "[{label}] mg must not write {rel}"
                );
                assert!(
                    !theirs.join(rel).exists(),
                    "[{label}] real git does not write {rel}"
                );
            }

            // 3) 逐字段对拍：`show-ref` / `symbolic-ref HEAD` / `status --porcelain` /
            //    `rev-parse --verify HEAD`（unborn 时两者都必须失败，且 stdout 都为空）。
            for (what, args) in [
                ("show-ref", vec!["show-ref"]),
                ("symbolic-ref HEAD", vec!["symbolic-ref", "HEAD"]),
                ("status --porcelain", vec!["status", "--porcelain"]),
                (
                    "rev-parse --verify HEAD",
                    vec!["rev-parse", "--verify", "HEAD"],
                ),
            ] {
                let ours_out = git_raw(&ours, &args);
                let theirs_out = git_raw(&theirs, &args);
                assert_eq!(
                    ours_out.status.code(),
                    theirs_out.status.code(),
                    "[{label}] `git {what}` exit code must match (mg: {})",
                    String::from_utf8_lossy(&ours_out.stderr)
                );
                assert_eq!(
                    String::from_utf8_lossy(&ours_out.stdout),
                    String::from_utf8_lossy(&theirs_out.stdout),
                    "[{label}] `git {what}` stdout must match"
                );
            }
            assert_eq!(git_text(&ours, &["status", "--porcelain"]), "");
            assert_eq!(
                git_text(&ours, &["symbolic-ref", "HEAD"]),
                "refs/heads/main"
            );
            for key in [
                "remote.origin.url",
                "remote.origin.fetch",
                "branch.main.remote",
                "branch.main.merge",
                "core.bare",
            ] {
                assert_eq!(
                    git_text(&ours, &["config", "--get", key]),
                    git_text(&theirs, &["config", "--get", key]),
                    "[{label}] config {key} must match a real git clone"
                );
            }
            assert_eq!(
                git_text(&ours, &["config", "--get", "remote.origin.url"]),
                file_url(&src)
            );
            // 真实 git 必须读得懂这个克隆（空仓库的 fsck 也是干净的）。
            git(&ours, &["fsck", "--no-progress", "--strict"]);
        }
    }

    /// 真实 `git fetch` 在空远端上 exit 0 且一个引用都不写；mg 必须一样 ——
    /// 通配 refspec 命中 0 个引用 → 静默成功；非通配 → `couldn't find remote ref`。
    #[test]
    fn fetch_from_an_empty_remote_succeeds_and_writes_nothing() {
        let (_origin_dir, empty) = empty_bare("empty-fetch.git");
        let url = file_url(&empty);

        let dir = tempfile::tempdir().unwrap();
        let ours = dir.path().join("ours");
        let repo = Repo::init(&ours, crate::repo::DEFAULT_INITIAL_BRANCH).unwrap();

        // 默认（通配）refspec：成功、0 引用、0 对象；FETCH_HEAD 被截成空文件（真实 git 同）。
        let outcome = fetch(&repo, &url, &[DEFAULT_FETCH_REFSPEC.to_string()])
            .expect("fetching an empty remote must not be fatal");
        assert!(outcome.refs.is_empty());
        assert!(outcome.head.is_none());
        assert_eq!(outcome.objects_written, 0);
        assert_eq!(RefStore::new(&repo).list().unwrap().len(), 0);
        assert_eq!(
            std::fs::read_to_string(ours.join(".git/FETCH_HEAD")).unwrap(),
            ""
        );

        // 真实 git 对同一夹具（**已配置的远端**）：exit 0、没有任何引用、FETCH_HEAD 空文件。
        let theirs = dir.path().join("theirs");
        std::fs::create_dir_all(&theirs).unwrap();
        git(&theirs, &["init", "-q", "-b", "main", "."]);
        git(&theirs, &["remote", "add", "origin", &url]);
        let real = git_raw(&theirs, &["fetch", "origin"]);
        assert_eq!(
            real.status.code(),
            Some(0),
            "real git fetch must succeed: {}",
            String::from_utf8_lossy(&real.stderr)
        );
        assert!(!git_raw(&theirs, &["show-ref"]).status.success());
        assert_eq!(
            std::fs::read_to_string(theirs.join(".git/FETCH_HEAD")).unwrap(),
            "",
            "real git leaves FETCH_HEAD empty (but present) after fetching an empty remote"
        );
        assert!(git_raw(&ours, &["show-ref"]).stdout.is_empty());

        // CLI 层：`mg fetch origin` 与真实 `git fetch origin` 一样**完全静默**（stdout/stderr 皆空）。
        git(&ours, &["remote", "add", "origin", &url]);
        let mg_out = Command::new(mg_bin())
            .args(["fetch", "origin"])
            .current_dir(&ours)
            .output()
            .expect("failed to spawn mg");
        assert_eq!(
            mg_out.status.code(),
            Some(0),
            "mg fetch must succeed: {}",
            String::from_utf8_lossy(&mg_out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&mg_out.stdout),
            String::from_utf8_lossy(&real.stdout),
            "a fetch that updates nothing must be as silent as real git's"
        );
        assert_eq!(
            String::from_utf8_lossy(&mg_out.stderr),
            String::from_utf8_lossy(&real.stderr)
        );

        // 非通配 refspec 在空远端上查不到 → 仍然是 fatal（真实 git 同）。
        let err = fetch(&repo, &url, &["refs/heads/main".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("couldn't find remote ref"),
            "unexpected error: {err}"
        );
        let real = git_raw(&theirs, &["fetch", &url, "refs/heads/main"]);
        assert!(
            !real.status.success(),
            "real git must fail on a missing non-wildcard ref"
        );
        let real_stderr = String::from_utf8_lossy(&real.stderr).to_string();
        assert!(
            real_stderr.contains("find remote ref"),
            "real git stderr: {real_stderr}"
        );
        assert!(git_raw(&ours, &["show-ref"]).stdout.is_empty());
    }

    /// 通配 refspec 匹配不到任何引用**不是**错误（D2 的根因）；
    /// 非通配的引用名匹配不到仍然是错误。
    #[test]
    fn fetch_tolerates_a_wildcard_refspec_that_matches_nothing() {
        let (_origin_dir, src) = make_origin("d2-wildcard", OriginTags::None, false);
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        let repo = clone_into(&file_url(&src), &dst).unwrap();
        assert_eq!(git_text(&dst, &["for-each-ref", "refs/tags/"]), "");

        // 显式再取一次 tag（通配命中 0 个）→ 成功、没有引用被写。
        let outcome = fetch(
            &repo,
            &file_url(&src),
            &["+refs/tags/*:refs/tags/*".to_string()],
        )
        .expect("an unmatched wildcard refspec must not be fatal");
        assert!(outcome.refs.is_empty());
        assert_eq!(outcome.objects_written, 0);

        // 非通配但不存在 → 仍然是 fatal（真实 git 同）。
        let err = fetch(&repo, &file_url(&src), &["refs/tags/nope".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("couldn't find remote ref"),
            "unexpected error: {err}"
        );
    }

    // ---------------------------------------------------------- fetch

    #[test]
    fn fetch_updates_remote_tracking_refs_and_fetch_head() {
        let (_src_dir, src) = make_src("fetch-src");
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        let repo = clone_into(&file_url(&src), &dst).unwrap();

        // 源仓库前进（新提交 + 新分支）。
        write(&src, "c.txt", b"three\n");
        commit(&src, "three");
        git(&src, &["branch", "third"]);

        let before = git_text(&dst, &["rev-parse", "refs/remotes/origin/main"]);
        let outcome = fetch(&repo, &file_url(&src), &[]).unwrap();
        assert!(outcome.objects_written > 0, "new objects must be written");
        assert_eq!(
            outcome.head.map(|(name, oid)| (name, oid.to_hex())),
            Some((
                "refs/heads/main".to_string(),
                git_text(&src, &["rev-parse", "HEAD"])
            ))
        );

        let after = git_text(&dst, &["rev-parse", "refs/remotes/origin/main"]);
        assert_ne!(before, after);
        assert_eq!(after, git_text(&src, &["rev-parse", "HEAD"]));
        assert_eq!(
            git_text(&dst, &["rev-parse", "refs/remotes/origin/third"]),
            git_text(&src, &["rev-parse", "refs/heads/third"])
        );
        // 本地分支与工作区在 fetch 后**不动**。
        assert_eq!(
            git_text(&dst, &["status", "--porcelain"]),
            "",
            "fetch must not touch the working tree"
        );

        // FETCH_HEAD：能被真实 `git show-ref`/`git rev-parse` 之外的工具读；
        // 这里直接检查第一行 oid 与远端 HEAD 一致，且带 not-for-merge 标记。
        let fetch_head = std::fs::read_to_string(dst.join(".git/FETCH_HEAD")).unwrap();
        let head_line = fetch_head
            .lines()
            .find(|line| line.contains("branch 'main'"))
            .expect("FETCH_HEAD must mention the remote HEAD branch");
        assert!(head_line.starts_with(&after));
        assert!(fetch_head.contains("not-for-merge"), "{fetch_head}");
        assert!(fetch_head.contains(&file_url(&src)), "{fetch_head}");

        // 再 fetch 一次：没有新对象。
        let again = fetch(&repo, &file_url(&src), &[]).unwrap();
        assert_eq!(again.objects_written, 0);

        // fetch 一个未知 refspec → 明确报错。
        assert!(fetch(&repo, &file_url(&src), &["refs/heads/nope".to_string()]).is_err());
    }

    // ---------------------------------------------------------- push

    #[test]
    fn push_is_readable_by_real_git_and_rejects_non_fast_forwards() {
        let (_src_dir, src) = make_src("push-src");
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        let repo = clone_into(&file_url(&src), &dst).unwrap();

        let (_remote_dir, remote) = empty_bare("remote.git");
        let url = file_url(&remote);

        // 1) 首次 push（远端为空 = 全 0 old）。
        push(&repo, &url, &["main:refs/heads/main".to_string()], false).unwrap();
        assert_eq!(
            git_text(&remote, &["rev-parse", "refs/heads/main"]),
            git_text(&src, &["rev-parse", "HEAD"])
        );
        git(&remote, &["fsck", "--no-progress"]);
        assert_eq!(
            git_text(&remote, &["log", "--oneline"]),
            git_text(&src, &["log", "--oneline"]),
            "real git must see the pushed history"
        );

        // 2) 本地提交 + 远端不动 → 正常 fast-forward push。
        write(&dst, "local.txt", b"local\n");
        git(&dst, &["add", "-A"]);
        git(&dst, &["commit", "-q", "-m", "local"]);
        push(&repo, &url, &[], false).unwrap();
        assert_eq!(
            git_text(&remote, &["rev-parse", "refs/heads/main"]),
            git_text(&dst, &["rev-parse", "HEAD"])
        );

        // 3) 让远端前进到一条**不是**本地祖先的历史 → non-fast-forward 必须被拒绝，
        //    且远端一个字节都不能变。
        let remote_before = git_text(&remote, &["rev-parse", "refs/heads/main"]);
        write(&src, "divergent.txt", b"divergent\n");
        commit(&src, "divergent");
        // 真实 git 从 src 推过去：src 落后于远端，所以要 --force 才能制造「分叉」状态。
        git(
            &src,
            &["push", "-q", "--force", &url, "main:refs/heads/main"],
        );
        let remote_advanced = git_text(&remote, &["rev-parse", "refs/heads/main"]);
        assert_ne!(remote_before, remote_advanced);

        let err = push(&repo, &url, &["main:refs/heads/main".to_string()], false).unwrap_err();
        assert!(
            err.to_string().contains("not a fast-forward"),
            "unexpected error: {err}"
        );
        assert_eq!(
            git_text(&remote, &["rev-parse", "refs/heads/main"]),
            remote_advanced,
            "a rejected push must leave the remote ref untouched"
        );

        // 4) --force 才改（真实 git 的 receive-pack 默认接受非 FF，与 --force 一致）。
        push(&repo, &url, &["main:refs/heads/main".to_string()], true).unwrap();
        assert_eq!(
            git_text(&remote, &["rev-parse", "refs/heads/main"]),
            git_text(&dst, &["rev-parse", "HEAD"])
        );
        git(&remote, &["fsck", "--no-progress"]);
    }

    #[test]
    fn push_refuses_the_checked_out_branch_of_a_non_bare_remote() {
        let (_src_dir, src) = make_src("push-nonbare-src");
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        let repo = clone_into(&file_url(&src), &dst).unwrap();

        // 一个**非 bare** 远端，当前检出 main。
        let (_remote_dir, remote) = make_src("push-nonbare-remote");
        let url = file_url(&remote);
        let remote_main = git_text(&remote, &["rev-parse", "refs/heads/main"]);
        git(&remote, &["branch", "dev"]);

        // 推 main（= 远端当前检出分支）→ 拒绝，远端不变（receive.denyCurrentBranch=refuse 的默认）。
        let err = push(&repo, &url, &["main:refs/heads/main".to_string()], true).unwrap_err();
        assert!(
            err.to_string().contains("currently checked out"),
            "unexpected error: {err}"
        );
        assert_eq!(
            git_text(&remote, &["rev-parse", "refs/heads/main"]),
            remote_main
        );

        // 推一个**没被检出**的分支 → 允许，真实 git 读得到。
        push(&repo, &url, &["main:refs/heads/dev".to_string()], true).unwrap();
        assert_eq!(
            git_text(&remote, &["rev-parse", "refs/heads/dev"]),
            git_text(&dst, &["rev-parse", "HEAD"])
        );
        git(&remote, &["fsck", "--no-progress"]);
    }

    #[test]
    fn set_branch_upstream_writes_config_visible_to_real_git() {
        let (_src_dir, src) = make_src("upstream-src");
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        let repo = clone_into(&file_url(&src), &dst).unwrap();

        set_branch_upstream(&repo, "main", "origin", "refs/heads/main").unwrap();
        assert_eq!(
            git_text(&dst, &["config", "--get", "branch.main.remote"]),
            "origin"
        );
        assert_eq!(
            git_text(&dst, &["config", "--get", "branch.main.merge"]),
            "refs/heads/main"
        );
        // mg 自己的 Config 也要能读回来。
        let config = repo.config();
        assert_eq!(config.get("branch.main.remote"), Some("origin"));
        assert_eq!(config.get("branch.main.merge"), Some("refs/heads/main"));
    }

    // ---------------------------------------------------------- 二进制端到端

    #[test]
    fn mg_binary_clone_fetch_push_pull_end_to_end() {
        let (_src_dir, src) = make_src("e2e-src");
        let dir = tempfile::tempdir().unwrap();

        // mg clone
        mg(dir.path(), &["clone", &file_url(&src), "dst"]);
        let dst = dir.path().join("dst");
        assert!(dst.join("a.txt").is_file());
        assert_eq!(git_text(&dst, &["fsck", "--no-progress"]), "");
        assert_eq!(
            git_text(&dst, &["log", "--oneline"]),
            git_text(&src, &["log", "--oneline"])
        );
        assert_eq!(git_text(&dst, &["status", "--porcelain"]), "");

        // mg fetch（源仓库前进后）
        write(&src, "fetched.txt", b"fetched\n");
        commit(&src, "fetched");
        mg(&dst, &["fetch"]);
        assert_eq!(
            git_text(&dst, &["rev-parse", "refs/remotes/origin/main"]),
            git_text(&src, &["rev-parse", "HEAD"])
        );

        // mg pull（fast-forward）
        mg(&dst, &["pull"]);
        assert_eq!(
            git_text(&dst, &["log", "--oneline"]),
            git_text(&src, &["log", "--oneline"])
        );
        assert_eq!(git_text(&dst, &["status", "--porcelain"]), "");
        assert_eq!(
            std::fs::read(dst.join("fetched.txt")).unwrap(),
            b"fetched\n"
        );
        assert_eq!(
            std::fs::read(dst.join("a.txt")).unwrap(),
            format!("one {}\ntwo\n", "e2e-src").as_bytes()
        );

        // mg push -u
        let (_remote_dir, remote) = empty_bare("e2e-remote.git");
        mg(&dst, &["push", "-u", &file_url(&remote), "main"]);
        assert_eq!(
            git_text(&remote, &["rev-parse", "refs/heads/main"]),
            git_text(&dst, &["rev-parse", "HEAD"])
        );
        assert_eq!(
            git_text(&remote, &["log", "--oneline"]),
            git_text(&src, &["log", "--oneline"])
        );
        git(&remote, &["fsck", "--no-progress"]);
        assert_eq!(
            git_text(&dst, &["config", "--get", "branch.main.remote"]),
            "origin"
        );

        // mg push 的非 FF 拒绝：退出码非 0、远端不变。
        write(&src, "divergent-e2e.txt", b"x\n");
        commit(&src, "divergent-e2e");
        git(
            &src,
            &["push", "-q", &file_url(&remote), "main:refs/heads/main"],
        );
        let advanced = git_text(&remote, &["rev-parse", "refs/heads/main"]);
        let out = Command::new(mg_bin())
            .args(["push", &file_url(&remote), "main"])
            .current_dir(&dst)
            .output()
            .unwrap();
        assert!(!out.status.success(), "a non-FF push must fail");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("fast-forward"),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            git_text(&remote, &["rev-parse", "refs/heads/main"]),
            advanced
        );

        // mg push --force 成功。
        mg(&dst, &["push", "--force", &file_url(&remote), "main"]);
        assert_eq!(
            git_text(&remote, &["rev-parse", "refs/heads/main"]),
            git_text(&dst, &["rev-parse", "HEAD"])
        );
    }

    #[test]
    fn mg_binary_reports_clear_errors() {
        let dir = tempfile::tempdir().unwrap();
        let out = Command::new(mg_bin())
            .args(["clone", "file:///nonexistent/mg-remote", "dst"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("does not exist"), "stderr: {stderr}");
        assert!(!dir.path().join("dst").exists());
        assert!(!stderr.contains("panicked"), "stderr: {stderr}");
    }

    #[test]
    fn upload_pack_response_side_band_channel_3_is_a_fatal_error() {
        let mut response = Vec::new();
        write_pkt(&mut response, b"NAK\n").unwrap();
        let mut error_band = vec![3u8];
        error_band.extend_from_slice(b"fatal: something\n");
        write_pkt(&mut response, &error_band).unwrap();
        write_flush(&mut response).unwrap();
        let err = parse_upload_pack_response(&response).unwrap_err();
        assert!(err.to_string().contains("something"), "{err}");

        // 没有 pack 数据也是错误（不许「空 pack 也算成功」）。
        let mut empty = Vec::new();
        write_pkt(&mut empty, b"NAK\n").unwrap();
        write_flush(&mut empty).unwrap();
        assert!(parse_upload_pack_response(&empty).is_err());
    }

    /// side-band 通道 2 是进度：必须被转发（这里打到 stderr）而**不能**混进 pack。
    #[test]
    fn side_band_progress_is_not_mixed_into_the_pack() {
        let mut pack = Vec::new();
        pack.extend_from_slice(b"PACK");
        pack.extend_from_slice(&2u32.to_be_bytes());
        pack.extend_from_slice(&0u32.to_be_bytes());
        let mut hasher = Sha1::new();
        hasher.update(&pack);
        pack.extend_from_slice(&hasher.finalize());

        let mut response = Vec::new();
        write_pkt(&mut response, b"NAK\n").unwrap();
        let mut progress = vec![2u8];
        progress.extend_from_slice(b"Enumerating objects: 3, done.\n");
        write_pkt(&mut response, &progress).unwrap();
        // pack 分两帧发（真实 git 会按 65515 字节切片）。
        let (head, tail) = pack.split_at(10);
        let mut first = vec![1u8];
        first.extend_from_slice(head);
        write_pkt(&mut response, &first).unwrap();
        let mut second = vec![1u8];
        second.extend_from_slice(tail);
        write_pkt(&mut response, &second).unwrap();
        write_flush(&mut response).unwrap();

        assert_eq!(parse_upload_pack_response(&response).unwrap(), pack);

        // ACK 行也是允许的前导帧（真实 git 在 multi_ack 下会发）。
        let mut acked = Vec::new();
        write_pkt(
            &mut acked,
            b"ACK 1111111111111111111111111111111111111111\n",
        )
        .unwrap();
        write_pkt(&mut acked, b"NAK\n").unwrap();
        let mut only = vec![1u8];
        only.extend_from_slice(&pack);
        write_pkt(&mut acked, &only).unwrap();
        write_flush(&mut acked).unwrap();
        assert_eq!(parse_upload_pack_response(&acked).unwrap(), pack);
    }

    /// 服务端只在客户端没要 `no-progress` 时发通道 2；两种情形下 pack 都能落地。
    #[test]
    fn upload_pack_respects_no_progress_and_installs_objects() {
        let (_dir, src) = make_src("progress-src");
        let repo = Repo::discover(&src).unwrap();
        let server = LocalRemote { repo: &repo };
        let head = RefStore::new(&repo).resolve("HEAD").unwrap();
        let closure = reachable(&repo, &[head], false).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        Repo::init(&dst, "main").unwrap();
        let dst_repo = Repo::discover(&dst).unwrap();

        for (caps, expect_progress, expect_written) in [
            (vec!["side-band-64k"], true, closure.len()),
            (vec!["side-band-64k", "no-progress"], false, 0),
        ] {
            let request = negotiate::build_fetch_request(
                &FetchRequest {
                    wants: vec![head],
                    haves: Vec::new(),
                    done: true,
                },
                &caps,
            );
            let response = server.upload_pack(&request).unwrap();
            let mut cursor = std::io::Cursor::new(response.as_slice());
            let mut pack = Vec::new();
            let mut saw_progress = false;
            loop {
                match crate::transport::pktline::read_pkt(&mut cursor).unwrap() {
                    Pkt::Data(data) => match data.first().copied() {
                        Some(1) => pack.extend_from_slice(&data[1..]),
                        Some(2) => saw_progress = true,
                        // 首帧是 `NAK\n`（真实 git 的 multi_ack 下还会先来 ACK 行）。
                        _ if data.starts_with(b"NAK") || data.starts_with(b"ACK") => {}
                        other => panic!("unexpected side-band channel {other:?}"),
                    },
                    Pkt::Flush => break,
                    other => panic!("unexpected frame {other:?}"),
                }
            }
            assert_eq!(saw_progress, expect_progress);
            assert_eq!(install_pack(&dst_repo, &pack).unwrap(), expect_written);
        }
        assert_eq!(
            Odb::new(&dst_repo).iter_loose().unwrap().len(),
            closure.len()
        );
    }

    #[test]
    fn url_parsing_covers_the_v1_surface() {
        assert!(matches!(locate("file:///tmp/x"), Ok(Location::File(_))));
        assert!(matches!(locate("/tmp/x"), Ok(Location::File(_))));
        assert!(matches!(locate("./x"), Ok(Location::File(_))));
        assert!(matches!(
            locate("file://localhost/tmp/x"),
            Ok(Location::File(_))
        ));
        assert!(matches!(
            locate("http://127.0.0.1:8080/x"),
            Ok(Location::Http(_))
        ));
        assert!(matches!(
            locate("https://example.com/x"),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            locate("git://example.com/x"),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            locate("file://host/tmp/x"),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(locate(""), Err(Error::Other(_))));
        if let Ok(Location::File(path)) = locate("file:///tmp/a%20b") {
            assert_eq!(path.to_str(), Some("/tmp/a b"));
        } else {
            panic!("percent-decoding failed");
        }
    }
}
