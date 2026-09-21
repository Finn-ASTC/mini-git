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
//!
//! # 与真实 git 的字节级对齐（T13 实测）
//!
//! 本文件的四个函数都只做「字节流 ↔ 结构体」，不含任何 socket / 文件访问；
//! 因此它们的真值直接来自**真实 git 进程**的字节：
//! * `parse_advertisement` 吃 `git upload-pack --advertise-refs` 的原始输出；
//! * `build_fetch_request` 的输出喂给 `git upload-pack --stateless-rpc` 必须被接受；
//! * `build_push_update` 的输出喂给 `git receive-pack` 必须被接受；
//! * `parse_report_status` 吃 `git receive-pack` 的原始输出。
//!
//! 见文件底部的差分测试。
//!
//! 两个刻意的宽松点（都为了兼容真实服务端，不是「大概就对」）：
//! * smart HTTP 的广告前面有 `# service=git-upload-pack` + flush，解析时直接跳过；
//! * 空仓库广告的是 `<40 个 0> capabilities^{}`（`symref` 也不出现），照原样收进 `refs`，
//!   由调用方判定「远端为空」，解析本身不算错误。

use std::io::Cursor;

use crate::error::{Error, Result};
use crate::oid::Oid;
use crate::transport::pktline::{read_pkt, write_flush, write_pkt, Pkt};

/// 空仓库广告里的占位引用名（git 的 `capabilities^{}` hack）。
pub const CAPABILITIES_REF: &str = "capabilities^{}";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefAdvertisement {
    pub refs: Vec<(String, Oid)>,
    pub capabilities: Vec<String>,
    /// 形如 `("HEAD", "refs/heads/main")`。
    pub symrefs: Vec<(String, String)>,
}

impl RefAdvertisement {
    /// 某个引用的 oid（`HEAD` 或全名）。
    pub fn get(&self, name: &str) -> Option<Oid> {
        self.refs
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, oid)| *oid)
    }

    pub fn has_capability(&self, name: &str) -> bool {
        self.capabilities.iter().any(|cap| cap == name)
    }

    /// `HEAD` 的符号引用目标（如 `refs/heads/main`）。
    pub fn head_symref(&self) -> Option<&str> {
        self.symrefs
            .iter()
            .find(|(name, _)| name == "HEAD")
            .map(|(_, target)| target.as_str())
    }

    /// 广告里是否有「真的引用」（排除空仓库的 `capabilities^{}` 占位）。
    pub fn is_empty_repository(&self) -> bool {
        !self
            .refs
            .iter()
            .any(|(name, oid)| name != CAPABILITIES_REF && !oid.is_zero())
    }
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

/// 解析 v0 引用广告（`upload-pack` / `receive-pack` 的输出，或 HTTP 的 `info/refs` 主体）。
///
/// 逐条读 pkt-line：数据帧是 `<oid> <name>`，首行在 NUL 之后带 capability 列表；
/// flush-pkt 结束。`# service=` 服务行与紧随其后的分帧结束（若在第一条引用之前）被跳过。
pub fn parse_advertisement(payload: &[u8]) -> Result<RefAdvertisement> {
    let mut cursor = Cursor::new(payload);
    let mut adv = RefAdvertisement::default();
    let mut saw_ref = false;

    loop {
        let pkt = match read_pkt(&mut cursor) {
            Ok(pkt) => pkt,
            Err(Error::Protocol(detail)) if !saw_ref && cursor.position() == 0 => {
                return Err(Error::Protocol(format!("ref advertisement: {detail}")))
            }
            Err(err) => return Err(err),
        };
        match pkt {
            Pkt::Data(data) => {
                let line = data.strip_suffix(b"\n").unwrap_or(&data);
                if line.is_empty() {
                    continue;
                }
                // smart HTTP：`# service=git-upload-pack` 服务行不是引用。
                if line.starts_with(b"#") {
                    continue;
                }
                let (ref_part, caps) = match line.iter().position(|&b| b == 0) {
                    Some(idx) => (&line[..idx], Some(&line[idx + 1..])),
                    None => (line, None),
                };
                if let Some(caps) = caps {
                    let caps = std::str::from_utf8(caps).map_err(|_| {
                        Error::Protocol("ref advertisement capabilities are not UTF-8".into())
                    })?;
                    for cap in caps.split(' ').filter(|cap| !cap.is_empty()) {
                        if let Some(symref) = cap.strip_prefix("symref=") {
                            if let Some((name, target)) = symref.split_once(':') {
                                adv.symrefs.push((name.to_string(), target.to_string()));
                            }
                        }
                        adv.capabilities.push(cap.to_string());
                    }
                }
                let space = ref_part.iter().position(|&b| b == b' ').ok_or_else(|| {
                    Error::Protocol(format!(
                        "ref advertisement line has no ref name: {:?}",
                        String::from_utf8_lossy(line)
                    ))
                })?;
                let oid =
                    Oid::from_hex(std::str::from_utf8(&ref_part[..space]).map_err(|_| {
                        Error::Protocol("ref advertisement oid is not ASCII".into())
                    })?)?;
                let name = std::str::from_utf8(&ref_part[space + 1..])
                    .map_err(|_| Error::Protocol("ref name is not UTF-8".into()))?
                    .trim()
                    .to_string();
                adv.refs.push((name, oid));
                saw_ref = true;
            }
            // 服务行之后的那个 flush 出现在第一条引用之前，跳过它。
            Pkt::Flush => {
                if saw_ref {
                    break;
                }
            }
            Pkt::Delim | Pkt::ResponseEnd => {
                return Err(Error::Protocol(
                    "unexpected delim/response-end pkt in a v0 ref advertisement".into(),
                ))
            }
        }
    }

    if adv.refs.is_empty() {
        return Err(Error::Protocol("ref advertisement lists no refs".into()));
    }
    Ok(adv)
}

/// 编码 v0 fetch 请求：`want` 行（首行带 caps）+ flush + `have` 行 + `done`。
pub fn build_fetch_request(req: &FetchRequest, caps: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for (idx, want) in req.wants.iter().enumerate() {
        let mut line = format!("want {}", want.to_hex());
        if idx == 0 && !caps.is_empty() {
            line.push(' ');
            line.push_str(&caps.join(" "));
        }
        line.push('\n');
        write_pkt(&mut out, line.as_bytes()).expect("writing to a Vec cannot fail");
    }
    if !req.wants.is_empty() {
        write_flush(&mut out).expect("writing to a Vec cannot fail");
    }
    for have in &req.haves {
        write_pkt(&mut out, format!("have {}\n", have.to_hex()).as_bytes())
            .expect("writing to a Vec cannot fail");
    }
    if req.done {
        write_pkt(&mut out, b"done\n").expect("writing to a Vec cannot fail");
    }
    out
}

/// 编码 v0 push 的 update 段：`<old> <new> <ref>`（首行 NUL 后带 caps）+ flush。
///
/// 返回的字节后面**紧跟裸 pack 数据**（push 的 pack 不打 pkt-line 分帧），由调用方拼接。
pub fn build_push_update(cmds: &[PushCommand], caps: &[&str]) -> Result<Vec<u8>> {
    if cmds.is_empty() {
        return Err(Error::Protocol(
            "a push request needs at least one ref update".into(),
        ));
    }
    let mut out = Vec::new();
    for (idx, cmd) in cmds.iter().enumerate() {
        if cmd.name.is_empty()
            || cmd
                .name
                .bytes()
                .any(|b| b <= b' ' || b == 0x7f || b == b'\\' || b == b'~' || b == b'^')
        {
            return Err(Error::Protocol(format!(
                "invalid ref name in a push update: {:?}",
                cmd.name
            )));
        }
        let mut line = format!("{} {} {}", cmd.old.to_hex(), cmd.new.to_hex(), cmd.name);
        if idx == 0 && !caps.is_empty() {
            line.push('\0');
            line.push_str(&caps.join(" "));
        }
        line.push('\n');
        write_pkt(&mut out, line.as_bytes())?;
    }
    write_flush(&mut out)?;
    Ok(out)
}

/// 解析 `report-status`，返回 `(ref, status)` 列表；`unpack` 失败也要能看到。
///
/// 约定：`unpack` 行记作 `("unpack", "ok" | <错误文本>)`，引用行记作
/// `("<refname>", "ok" | "ng <reason>")`。顺序与线上一致（`unpack` 在最前）。
pub fn parse_report_status(payload: &[u8]) -> Result<Vec<(String, String)>> {
    let mut cursor = Cursor::new(payload);
    let mut out = Vec::new();
    loop {
        match read_pkt(&mut cursor)? {
            Pkt::Data(data) => {
                let line = data.strip_suffix(b"\n").unwrap_or(&data);
                let text = std::str::from_utf8(line)
                    .map_err(|_| Error::Protocol("report-status line is not UTF-8".into()))?;
                if let Some(rest) = text.strip_prefix("unpack ") {
                    out.push(("unpack".to_string(), rest.to_string()));
                } else if let Some(rest) = text.strip_prefix("ok ") {
                    out.push((rest.to_string(), "ok".to_string()));
                } else if let Some(rest) = text.strip_prefix("ng ") {
                    let (name, reason) = rest.split_once(' ').unwrap_or((rest, ""));
                    out.push((name.to_string(), format!("ng {reason}")));
                } else {
                    return Err(Error::Protocol(format!(
                        "unrecognized report-status line: {text:?}"
                    )));
                }
            }
            Pkt::Flush => break,
            Pkt::Delim | Pkt::ResponseEnd => {
                return Err(Error::Protocol(
                    "unexpected delim/response-end pkt in report-status".into(),
                ))
            }
        }
    }
    if out.is_empty() {
        return Err(Error::Protocol("empty report-status".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    //! 真值全部来自**真实 git 进程**的字节（`upload-pack --advertise-refs`、
    //! `upload-pack --stateless-rpc`、`receive-pack`）。
    //!
    //! `git` 不存在时**直接 panic**（不允许「环境缺失 → 静默通过」）。

    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};

    use super::*;
    use crate::transport::pktline::{read_pkt, write_flush, write_pkt, Pkt};

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    fn git(dir: &Path, args: &[&str]) -> Output {
        assert!(git_available(), "this test needs a real `git` on PATH");
        let output = Command::new("git")
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
            .expect("failed to spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        output
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

    /// 一个有 2 个提交、一个分支、一个附注 tag、一个轻量 tag 的真实仓库。
    fn fixture(label: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = dir.path().join(label);
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", "."]);
        // 注意：子目录，用来覆盖嵌套 tree 的广告 / 请求路径。
        write(&work, "a.txt", b"one\n");
        write(&work, "sub/b.txt", b"two\n");
        commit(&work, "one");
        write(&work, "a.txt", b"one\ntwo\n");
        commit(&work, "two");
        git(&work, &["branch", "feature"]);
        git(&work, &["tag", "-a", "v1", "-m", "tag message"]);
        git(&work, &["tag", "light"]);
        let git_dir = work.join(".git");
        (dir, git_dir)
    }

    fn advertise(git_dir: &Path) -> Vec<u8> {
        git(
            git_dir,
            &[
                "-c",
                "protocol.version=0",
                "upload-pack",
                "--advertise-refs",
                ".",
            ],
        )
        .stdout
    }

    /// 手工分帧：`<4 位小写十六进制长度><payload>`（不借助被测的 encode_pkt）。
    fn frame(payload: &[u8]) -> Vec<u8> {
        let mut out = format!("{:04x}", payload.len() + 4).into_bytes();
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn parses_real_advertisement_and_reencodes_it_byte_for_byte() {
        let (dir, git_dir) = fixture("adv");
        let raw = advertise(&git_dir);
        let adv = parse_advertisement(&raw).expect("real advertisement must parse");

        assert_eq!(
            adv.get("HEAD").map(|oid| oid.to_hex()),
            Some(git_text(&git_dir, &["rev-parse", "HEAD"]))
        );
        assert_eq!(
            adv.get("refs/heads/main").map(|oid| oid.to_hex()),
            Some(git_text(&git_dir, &["rev-parse", "refs/heads/main"]))
        );
        assert_eq!(adv.head_symref(), Some("refs/heads/main"));
        assert!(
            adv.has_capability("side-band-64k"),
            "{:?}",
            adv.capabilities
        );
        assert!(adv.has_capability("ofs-delta"));
        assert!(!adv.is_empty_repository());
        // 附注 tag 的剥离行也在广告里（真实 git 会发）。
        assert!(
            adv.refs.iter().any(|(name, _)| name == "refs/tags/v1^{}"),
            "advertisement must carry the peeled line: {:?}",
            adv.refs.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
        assert_eq!(
            adv.get("refs/tags/v1^{}").map(|oid| oid.to_hex()),
            Some(git_text(&git_dir, &["rev-parse", "v1^{}"])),
        );

        // 逐帧重新编码：必须与真实 git 的字节**完全一致**（含首行的 \0 caps 与结尾 flush）。
        let mut rebuilt = Vec::new();
        for (idx, (name, oid)) in adv.refs.iter().enumerate() {
            let mut line = format!("{} {name}", oid.to_hex());
            if idx == 0 {
                line.push('\0');
                line.push_str(&adv.capabilities.join(" "));
            }
            line.push('\n');
            write_pkt(&mut rebuilt, line.as_bytes()).unwrap();
        }
        write_flush(&mut rebuilt).unwrap();
        assert_eq!(
            rebuilt, raw,
            "re-encoding the parsed advertisement must reproduce git's bytes"
        );

        // 每一帧都必须是合法 pkt-line，且首帧是 `HEAD\0caps`。
        let mut cursor = Cursor::new(raw.as_slice());
        let first = match read_pkt(&mut cursor).unwrap() {
            Pkt::Data(data) => data,
            other => panic!("first advertisement frame must be data, got {other:?}"),
        };
        assert_eq!(
            &first[..45 + 1],
            format!("{} HEAD\0", adv.get("HEAD").unwrap().to_hex()).as_bytes()
        );
        assert!(matches!(
            read_pkt(&mut Cursor::new(raw.as_slice())).unwrap(),
            Pkt::Data(_)
        ));
        let _ = dir;
    }

    #[test]
    fn empty_repository_uses_the_capabilities_hack() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("empty");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", "."]);
        let raw = advertise(&work.join(".git"));

        let marker = format!("{} capabilities^{{}}", Oid::zeros().to_hex());
        assert!(
            raw.windows(marker.len())
                .any(|window| window == marker.as_bytes()),
            "an empty repository advertises the capabilities hack"
        );

        let adv = parse_advertisement(&raw).expect("empty advertisement must parse");
        assert!(adv.is_empty_repository());
        assert_eq!(adv.refs, vec![(CAPABILITIES_REF.to_string(), Oid::zeros())]);
        assert_eq!(adv.head_symref(), None, "empty repos advertise no symref");
        assert!(adv.has_capability("side-band-64k"));
    }

    #[test]
    fn fetch_request_bytes_are_exact() {
        let want = Oid::from_hex(&"11".repeat(20)).unwrap();
        let want2 = Oid::from_hex(&"22".repeat(20)).unwrap();
        let have = Oid::from_hex(&"33".repeat(20)).unwrap();

        let out = build_fetch_request(
            &FetchRequest {
                wants: vec![want, want2],
                haves: vec![have],
                done: true,
            },
            &["side-band-64k", "ofs-delta"],
        );
        let mut expected = Vec::new();
        expected.extend_from_slice(&frame(
            format!("want {} side-band-64k ofs-delta\n", want.to_hex()).as_bytes(),
        ));
        expected.extend_from_slice(&frame(format!("want {}\n", want2.to_hex()).as_bytes()));
        expected.extend_from_slice(b"0000");
        expected.extend_from_slice(&frame(format!("have {}\n", have.to_hex()).as_bytes()));
        expected.extend_from_slice(&frame(b"done\n"));
        assert_eq!(out, expected);

        // 没有 want 时不发 flush（避免服务端读到空请求的 `done`）。
        let out = build_fetch_request(
            &FetchRequest {
                wants: Vec::new(),
                haves: Vec::new(),
                done: true,
            },
            &["side-band-64k"],
        );
        assert_eq!(out, frame(b"done\n"));
    }

    #[test]
    fn push_update_bytes_are_exact_and_validated() {
        let old = Oid::from_hex(&"aa".repeat(20)).unwrap();
        let new = Oid::from_hex(&"bb".repeat(20)).unwrap();
        let cmds = vec![
            PushCommand {
                old: Oid::zeros(),
                new,
                name: "refs/heads/main".to_string(),
            },
            PushCommand {
                old,
                new,
                name: "refs/heads/dev".to_string(),
            },
        ];
        let out = build_push_update(&cmds, &["report-status"]).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&frame(
            format!(
                "{} {} refs/heads/main\0report-status\n",
                Oid::zeros().to_hex(),
                new.to_hex()
            )
            .as_bytes(),
        ));
        expected.extend_from_slice(&frame(
            format!("{} {} refs/heads/dev\n", old.to_hex(), new.to_hex()).as_bytes(),
        ));
        expected.extend_from_slice(b"0000");
        assert_eq!(out, expected);

        assert!(build_push_update(&[], &["report-status"]).is_err());
        let bad = vec![PushCommand {
            old: Oid::zeros(),
            new,
            name: "refs/heads/bad name".to_string(),
        }];
        assert!(build_push_update(&bad, &[]).is_err());
    }

    /// 我们写出的 fetch 请求必须被**真实 git** 接受，且 side-band 通道 1 里的 pack
    /// 必须能被 `git index-pack` 验证通过。
    #[test]
    fn real_upload_pack_accepts_our_request_and_the_pack_verifies() {
        let (_dir, git_dir) = fixture("fetch");
        let adv = parse_advertisement(&advertise(&git_dir)).unwrap();
        let head = adv.get("HEAD").expect("HEAD advertised");

        let request = build_fetch_request(
            &FetchRequest {
                wants: vec![head],
                haves: Vec::new(),
                done: true,
            },
            &["side-band-64k", "no-progress"],
        );

        let mut child = Command::new("git")
            .arg("-C")
            .arg(&git_dir)
            .args([
                "-c",
                "protocol.version=0",
                "upload-pack",
                "--stateless-rpc",
                ".",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&request).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git rejected our fetch request: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let mut cursor = Cursor::new(out.stdout.as_slice());
        let first = match read_pkt(&mut cursor).unwrap() {
            Pkt::Data(data) => data,
            other => panic!("first response frame must be data, got {other:?}"),
        };
        assert!(
            first.starts_with(b"NAK") || first.starts_with(b"ACK"),
            "first frame should be NAK/ACK, got {:?}",
            String::from_utf8_lossy(&first)
        );
        let mut pack = Vec::new();
        let mut saw_progress = false;
        loop {
            match read_pkt(&mut cursor).expect("response must be valid pkt-line") {
                Pkt::Data(data) => match data.first().copied() {
                    Some(1) => pack.extend_from_slice(&data[1..]),
                    Some(2) => saw_progress = true,
                    Some(3) => panic!("git reported a fatal error on band 3"),
                    _ => panic!(
                        "non-band frame after NAK: {:?}",
                        String::from_utf8_lossy(&data)
                    ),
                },
                Pkt::Flush => break,
                other => panic!("unexpected frame {other:?}"),
            }
        }
        assert!(!saw_progress, "we asked for no-progress");
        assert!(
            pack.starts_with(b"PACK"),
            "side-band channel 1 must hold a pack"
        );

        // 让真实 git 验证这个 pack（含尾部 sha1 与全部 delta 链）。
        let mut index_child = Command::new("git")
            .arg("-C")
            .arg(&git_dir)
            .args(["index-pack", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        index_child.stdin.take().unwrap().write_all(&pack).unwrap();
        let indexed = index_child.wait_with_output().unwrap();
        assert!(
            indexed.status.success(),
            "git index-pack rejected the pack we received: {}",
            String::from_utf8_lossy(&indexed.stderr)
        );
        // `git index-pack --stdin` 打印 `pack\t<40 hex sha1>`：pack 的 sha1 由 git 自己算出来，
        // 说明它接受了我们收到的每一个字节。
        let stdout = String::from_utf8_lossy(&indexed.stdout);
        let pack_sha = stdout.split_whitespace().last().unwrap_or_default();
        assert_eq!(pack_sha.len(), 40, "index-pack output: {stdout:?}");
        assert!(pack_sha.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    /// 我们写出的 push update + pack 必须被**真实 git receive-pack** 接受，
    /// 且它的 report-status 必须能被我们解析。
    ///
    /// `receive-pack` 会先发一遍自己的广告（真实 `git push` 也是先读广告再发请求），
    /// 所以这里按帧读到 flush、再写请求 —— 顺带用真实字节再验证一遍
    /// `parse_advertisement`（空仓库的 `capabilities^{}` 形态）。
    #[test]
    fn real_receive_pack_accepts_our_push_and_report_status_parses() {
        let (_dir, git_dir) = fixture("push");
        let work = git_dir.parent().unwrap();
        let bare = dir_of(&git_dir).join("bare.git");
        git(
            work,
            &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
        );

        let head = git_text(&git_dir, &["rev-parse", "HEAD"]);
        let pack = git(work, &["pack-objects", "--stdout", "--all"]).stdout;
        assert!(pack.starts_with(b"PACK"));

        let cmds = vec![PushCommand {
            old: Oid::zeros(),
            new: Oid::from_hex(&head).unwrap(),
            name: "refs/heads/main".to_string(),
        }];
        let mut request = build_push_update(&cmds, &["report-status"]).unwrap();
        request.extend_from_slice(&pack);

        let mut child = Command::new("git")
            .arg("-C")
            .arg(&bare)
            .args(["-c", "protocol.version=0", "receive-pack", "."])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());

        // 1) 真实 receive-pack 的广告：逐帧读，再按帧原样重编码成字节流。
        let mut advertisement = Vec::new();
        loop {
            match read_pkt(&mut stdout).expect("advertisement must be valid pkt-line") {
                Pkt::Data(data) => write_pkt(&mut advertisement, &data).unwrap(),
                Pkt::Flush => {
                    write_flush(&mut advertisement).unwrap();
                    break;
                }
                other => panic!("unexpected frame in advertisement: {other:?}"),
            }
        }
        let adv = parse_advertisement(&advertisement).expect("receive-pack advertisement parses");
        assert!(adv.is_empty_repository());
        assert!(adv.has_capability("report-status") || adv.has_capability("side-band-64k"));

        // 2) 写请求 → 关掉 stdin → 读完整响应。
        stdin.write_all(&request).unwrap();
        drop(stdin);

        let mut rest = Vec::new();
        std::io::Read::read_to_end(&mut stdout, &mut rest).unwrap();
        let child_status = child.wait().unwrap();
        let stderr = String::from_utf8_lossy(
            &std::io::Read::bytes(child.stderr.take().unwrap())
                .collect::<std::result::Result<Vec<u8>, _>>()
                .unwrap(),
        )
        .to_string();

        assert!(
            child_status.success(),
            "receive-pack must report success: stderr={stderr}"
        );
        let status = parse_report_status(&rest)
            .unwrap_or_else(|err| panic!("real report-status must parse ({err}): stderr={stderr}"));
        assert_eq!(
            status,
            vec![
                ("unpack".to_string(), "ok".to_string()),
                ("refs/heads/main".to_string(), "ok".to_string())
            ]
        );
        assert_eq!(
            git_text(&bare, &["rev-parse", "refs/heads/main"]),
            head,
            "the pushed ref must be visible to real git"
        );
        git(&bare, &["fsck", "--no-progress"]);
    }

    fn dir_of(git_dir: &Path) -> PathBuf {
        git_dir.parent().unwrap().to_path_buf()
    }

    #[test]
    fn report_status_shapes_and_errors() {
        let mut payload = Vec::new();
        write_pkt(&mut payload, b"unpack ok\n").unwrap();
        write_pkt(&mut payload, b"ok refs/heads/main\n").unwrap();
        write_pkt(&mut payload, b"ng refs/heads/dev non-fast-forward\n").unwrap();
        write_flush(&mut payload).unwrap();
        assert_eq!(
            parse_report_status(&payload).unwrap(),
            vec![
                ("unpack".to_string(), "ok".to_string()),
                ("refs/heads/main".to_string(), "ok".to_string()),
                (
                    "refs/heads/dev".to_string(),
                    "ng non-fast-forward".to_string()
                ),
            ]
        );

        let mut failed = Vec::new();
        write_pkt(&mut failed, b"unpack index-pack failed\n").unwrap();
        write_pkt(&mut failed, b"ng refs/heads/main unpacker error\n").unwrap();
        write_flush(&mut failed).unwrap();
        let parsed = parse_report_status(&failed).unwrap();
        assert_eq!(
            parsed[0],
            ("unpack".to_string(), "index-pack failed".to_string())
        );
        assert_eq!(
            parsed[1],
            (
                "refs/heads/main".to_string(),
                "ng unpacker error".to_string()
            )
        );

        assert!(
            parse_report_status(b"0000").is_err(),
            "empty report is an error"
        );
        assert!(parse_report_status(b"garbage").is_err());
        let mut unknown = Vec::new();
        write_pkt(&mut unknown, b"whatever\n").unwrap();
        assert!(parse_report_status(&unknown).is_err());
    }

    #[test]
    fn advertisement_error_paths() {
        assert!(parse_advertisement(b"").is_err());
        assert!(parse_advertisement(b"0000").is_err(), "no refs at all");
        // 截断：长度前缀说 0x20 字节，实际只有几字节。
        assert!(parse_advertisement(b"0020abc").is_err());
        // 非十六进制 oid。
        let mut bad = Vec::new();
        write_pkt(
            &mut bad,
            b"zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz HEAD\0side-band-64k\n",
        )
        .unwrap();
        write_flush(&mut bad).unwrap();
        assert!(parse_advertisement(&bad).is_err());
        // 没有 ref 名。
        let mut bad_name = Vec::new();
        write_pkt(&mut bad_name, "0".repeat(40).as_bytes()).unwrap();
        write_flush(&mut bad_name).unwrap();
        assert!(parse_advertisement(&bad_name).is_err());
        // smart HTTP 的 `# service=...` 行 + 它后面的 flush 要被跳过。
        let mut http = Vec::new();
        write_pkt(&mut http, b"# service=git-upload-pack\n").unwrap();
        write_flush(&mut http).unwrap();
        write_pkt(
            &mut http,
            format!("{} HEAD\0side-band-64k\n", "ab".repeat(20)).as_bytes(),
        )
        .unwrap();
        write_flush(&mut http).unwrap();
        let adv = parse_advertisement(&http).unwrap();
        assert_eq!(adv.refs.len(), 1);
        assert_eq!(adv.get("HEAD").unwrap().to_hex(), "ab".repeat(20));
    }
}
