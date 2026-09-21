//! V8 —— T8（pkt-line 分帧）的**独立验证**（验证者：hermes）。
//!
//! 原则：真值只能来自 **真实 `git` 进程产出的字节流**，或来自协议规格本身。
//! 本文件不使用 `encode_pkt` 自造 fixture 去喂 `read_pkt` 当作「正确性」证据
//! （那只能证明自洽）。凡是自造 fixture，都用本文件自己的 `spec_data`
//! （独立按格式表 `<4 位小写十六进制长度><payload>` 实现）而不是 `encode_pkt`。
//!
//! 只使用公开 API：`minigit::transport::pktline::{read_pkt, write_pkt, write_flush,
//! encode_pkt, Pkt, MAX_PAYLOAD_LEN}`。

use std::collections::BTreeSet;
use std::io::{Cursor, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use minigit::transport::pktline::{
    encode_pkt, read_pkt, write_flush, write_pkt, Pkt, MAX_PAYLOAD_LEN,
};
use minigit::Error;

// ---------------------------------------------------------------------------
// 独立 oracle：按 git 的格式表切片，完全不使用被测实现
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawFrame {
    Data(Vec<u8>),
    Flush,
    Delim,
    ResponseEnd,
}

fn hex4(buf: &[u8]) -> usize {
    assert_eq!(buf.len(), 4, "oracle: 长度前缀必须是 4 字节");
    buf.iter().fold(0usize, |acc, &c| {
        acc * 16
            + (c as char)
                .to_digit(16)
                .unwrap_or_else(|| panic!("oracle: 非十六进制长度字节 {c:02x}"))
                as usize
    })
}

/// 验证者的独立分帧器（oracle）。若输入不合法就会 panic —— 只用于真实 git 的输出。
fn oracle_frames(raw: &[u8]) -> Vec<RawFrame> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off < raw.len() {
        assert!(
            off + 4 <= raw.len(),
            "oracle: 流在 {off} 处只剩 {} 字节，读不满 4 字节长度前缀",
            raw.len() - off
        );
        let len = hex4(&raw[off..off + 4]);
        match len {
            0 => {
                out.push(RawFrame::Flush);
                off += 4;
            }
            1 => {
                out.push(RawFrame::Delim);
                off += 4;
            }
            2 => {
                out.push(RawFrame::ResponseEnd);
                off += 4;
            }
            n => {
                assert!(n >= 4, "oracle: 帧长 {n} < 4（{off} 处）");
                assert!(
                    off + n <= raw.len(),
                    "oracle: 帧在 {off} 处越过流尾（声明 {n}，只剩 {}）",
                    raw.len() - off
                );
                out.push(RawFrame::Data(raw[off + 4..off + n].to_vec()));
                off += n;
            }
        }
    }
    out
}

/// 用**被测** `read_pkt` 走完整个流，返回帧序列与每帧的游标区间 `[start, end)`。
fn read_all(raw: &[u8]) -> (Vec<Pkt>, Vec<(usize, usize)>) {
    let mut cursor = Cursor::new(raw);
    let mut pkts = Vec::new();
    let mut spans = Vec::new();
    while (cursor.position() as usize) < raw.len() {
        let start = cursor.position() as usize;
        let pkt = read_pkt(&mut cursor)
            .unwrap_or_else(|err| panic!("read_pkt 在偏移 {start} 处拒绝了真实数据: {err:?}"));
        let end = cursor.position() as usize;
        assert!(
            end > start,
            "read_pkt 在偏移 {start} 处没有前进（死循环/零消费）"
        );
        assert!(
            end <= raw.len(),
            "read_pkt 从 {start} 读到 {end}，越过了流的实际长度 {}",
            raw.len()
        );
        spans.push((start, end));
        pkts.push(pkt);
    }
    (pkts, spans)
}

// ---------------------------------------------------------------------------
// 独立按格式表构造 fixture（刻意不用 encode_pkt）
// ---------------------------------------------------------------------------

fn spec_header(len: usize) -> [u8; 4] {
    assert!(len <= 0xffff);
    let d = b"0123456789abcdef";
    [
        d[(len >> 12) & 0xf],
        d[(len >> 8) & 0xf],
        d[(len >> 4) & 0xf],
        d[len & 0xf],
    ]
}

fn spec_data(payload: &[u8]) -> Vec<u8> {
    let mut out = spec_header(payload.len() + 4).to_vec();
    out.extend_from_slice(payload);
    out
}

// ---------------------------------------------------------------------------
// 真实 git 辅助
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("V8 验证需要 PATH 上有真实 git")
}

fn git_ok(dir: &Path, args: &[&str]) -> Output {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {:?} 失败: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn head_oid(repo: &Path) -> String {
    let out = git_ok(repo, &["rev-parse", "HEAD"]);
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// 建一个真仓库 + 一次真提交（身份只用 `-c` 限定到单条命令，不 export 任何 GIT_* 变量）。
fn init_repo_with_commit() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git_ok(
        dir.path(),
        &["-c", "init.defaultBranch=main", "init", "-q", "."],
    );
    std::fs::write(dir.path().join("a.txt"), b"hello v8\n").unwrap();
    git_ok(dir.path(), &["add", "a.txt"]);
    git_ok(
        dir.path(),
        &[
            "-c",
            "user.name=v8-hermes",
            "-c",
            "user.email=v8@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "c1",
        ],
    );
    dir
}

fn advertise_refs_v0(gitdir: &Path) -> Vec<u8> {
    let out = git_ok(
        gitdir.parent().unwrap(),
        &[
            "-c",
            "protocol.version=0",
            "upload-pack",
            "--stateless-rpc",
            "--advertise-refs",
            gitdir.to_str().unwrap(),
        ],
    );
    out.stdout
}

// ---------------------------------------------------------------------------
// (B)1 硬标准：解析真实 git 的 v0 ref advertisement
// ---------------------------------------------------------------------------

#[test]
fn real_git_v0_advertisement_roundtrips_byte_for_byte() {
    let repo = init_repo_with_commit();
    let gitdir = repo.path().join(".git");
    let raw = advertise_refs_v0(&gitdir);
    assert!(!raw.is_empty(), "真实 git 的 advertisement 不该为空");

    // 独立 oracle 先切一遍：输入本身必须与格式表自洽
    let oracle = oracle_frames(&raw);
    assert!(oracle.len() >= 3, "应有 >=2 个 Data 帧 + flush: {oracle:?}");
    assert_eq!(oracle.last(), Some(&RawFrame::Flush));

    // 被测实现逐帧读，游标必须与 oracle 的分界完全一致
    let (pkts, spans) = read_all(&raw);
    assert_eq!(
        pkts.len(),
        oracle.len(),
        "read_pkt 切出 {} 帧，oracle 切出 {} 帧",
        pkts.len(),
        oracle.len()
    );
    let mut rebuilt = Vec::new();
    let mut expected_off = 0usize;
    for (i, (pkt, &(start, end))) in pkts.iter().zip(spans.iter()).enumerate() {
        assert_eq!(start, expected_off, "第 {i} 帧起点与 oracle 的分界不符");
        assert_eq!(
            end - start,
            match pkt {
                Pkt::Data(d) => d.len() + 4,
                _ => 4,
            },
            "第 {i} 帧消费的字节数不对"
        );
        expected_off = end;
        match (pkt, &oracle[i]) {
            (Pkt::Data(d), RawFrame::Data(want)) => {
                assert_eq!(d, want, "第 {i} 帧 payload 与 oracle 不符");
                // 逐字节重建：encode_pkt(payload) 必须等于原始那一段字节
                assert_eq!(
                    encode_pkt(d).as_slice(),
                    &raw[start..end],
                    "第 {i} 帧无法用 encode_pkt 逐字节重建"
                );
                rebuilt.extend_from_slice(&encode_pkt(d));
            }
            (Pkt::Flush, RawFrame::Flush) => {
                assert_eq!(&raw[start..end], b"0000");
                rebuilt.extend_from_slice(b"0000");
            }
            (Pkt::Delim, RawFrame::Delim) => {
                assert_eq!(&raw[start..end], b"0001");
                rebuilt.extend_from_slice(b"0001");
            }
            (Pkt::ResponseEnd, RawFrame::ResponseEnd) => {
                assert_eq!(&raw[start..end], b"0002");
                rebuilt.extend_from_slice(b"0002");
            }
            (pkt, want) => panic!("第 {i} 帧类型不符: read_pkt={pkt:?} oracle={want:?}"),
        }
    }
    assert_eq!(rebuilt, raw, "所有帧重编码后必须与 git 原始字节流一致");
    assert_eq!(expected_off, raw.len(), "必须正好消费完 git 的全部输出");
    assert_eq!(pkts.last(), Some(&Pkt::Flush), "最后一帧必须是 Flush");

    // 帧切错了就绝不可能解析出 ref，更不可能与真实 oid 相等
    let mut refs: Vec<(String, String)> = Vec::new();
    for pkt in &pkts {
        if let Pkt::Data(d) = pkt {
            // v0 广告的一帧 = `<oid> <refname>`，第一帧 refname 后跟 NUL + capabilities
            let line = match d.iter().position(|&b| b == 0) {
                Some(i) => &d[..i],
                None => &d[..],
            };
            let mut it = line.split(|&b| b == b' ').filter(|s| !s.is_empty());
            let oid = it.next().unwrap_or_default();
            let name = it.next().unwrap_or_default();
            assert_eq!(
                oid.len(),
                40,
                "帧未对齐到 ref 行: {:?}",
                String::from_utf8_lossy(d)
            );
            assert!(oid.iter().all(u8::is_ascii_hexdigit));
            refs.push((
                String::from_utf8(oid.to_vec()).unwrap(),
                String::from_utf8_lossy(name).trim().to_string(),
            ));
        }
    }
    assert!(
        refs.iter().any(|(_, n)| n == "HEAD"),
        "未解析出 HEAD: {refs:?}"
    );
    assert!(
        refs.iter().any(|(_, n)| n == "refs/heads/main"),
        "未解析出 refs/heads/main: {refs:?}"
    );
    let want_oid = head_oid(repo.path());
    for (oid, name) in &refs {
        assert_eq!(
            oid, &want_oid,
            "ref {name} 的 oid 与真实仓库不符（帧切错了？）"
        );
    }
    // 第一帧携带 capabilities（NUL 之后），其余帧不应有 NUL
    match &pkts[0] {
        Pkt::Data(d) => assert!(
            d.contains(&0),
            "广告首帧必须含 refname 后的 NUL + capabilities"
        ),
        other => panic!("首帧应为 Data，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// (B)3 写方向 + side-band：把 mg 写的 request 喂给真实 git，再验证返回的 pack
// ---------------------------------------------------------------------------

#[test]
fn written_request_is_accepted_by_real_git_and_rebuilt_pack_is_valid() {
    let repo = init_repo_with_commit();
    let gitdir = repo.path().join(".git");
    let head = head_oid(repo.path());

    let want_line = format!("want {head} side-band-64k\n");
    let mut req = Vec::new();
    write_pkt(&mut req, want_line.as_bytes()).unwrap();
    write_flush(&mut req).unwrap();
    write_pkt(&mut req, b"done\n").unwrap();

    // 写出的字节必须能被 read_pkt 原样读回
    let mut c = Cursor::new(req.as_slice());
    assert_eq!(
        read_pkt(&mut c).unwrap(),
        Pkt::Data(want_line.clone().into_bytes())
    );
    assert_eq!(read_pkt(&mut c).unwrap(), Pkt::Flush);
    assert_eq!(read_pkt(&mut c).unwrap(), Pkt::Data(b"done\n".to_vec()));
    assert_eq!(c.position() as usize, req.len(), "写出的流应被正好读完");

    // 真实 git 必须接受「Data + flush + Data」这种形态（无协议错）
    let mut child = Command::new("git")
        .args(["-c", "protocol.version=0", "upload-pack", "--stateless-rpc"])
        .arg(&gitdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&req).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "真实 git 拒绝了 mg 写出的 request: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.stdout.is_empty(), "upload-pack 应有 pkt-line 响应");

    // 响应必须能被 read_pkt 逐帧读净，游标连续无重叠
    let (pkts, spans) = read_all(&out.stdout);
    assert_eq!(spans.first().unwrap().0, 0);
    assert_eq!(spans.last().unwrap().1, out.stdout.len());
    for w in spans.windows(2) {
        assert_eq!(w[0].1, w[1].0, "帧之间必须无缝衔接: {spans:?}");
    }
    match &pkts[0] {
        Pkt::Data(d) => assert!(
            d.starts_with(b"NAK") || d.starts_with(b"ACK"),
            "首帧应为 NAK/ACK，实际 {:?}",
            String::from_utf8_lossy(d)
        ),
        other => panic!("首帧应为 Data，实际 {other:?}"),
    }
    assert_eq!(pkts.last(), Some(&Pkt::Flush));

    // side-band 通道字节原样保留；通道 1 的负载拼起来必须是一个合法 pack
    let mut channels: BTreeSet<u8> = BTreeSet::new();
    let mut pack = Vec::new();
    for pkt in &pkts {
        if let Pkt::Data(d) = pkt {
            if let Some(&ch) = d.first() {
                channels.insert(ch);
                if ch == 1 {
                    pack.extend_from_slice(&d[1..]);
                }
            }
        }
    }
    assert!(channels.contains(&1), "缺少 side-band 通道 1: {channels:?}");
    assert!(channels.contains(&2), "缺少 side-band 通道 2: {channels:?}");
    assert_eq!(&pack[..4], b"PACK", "通道 1 的负载应以 PACK magic 开头");

    // 真值判决：把重建出来的 pack 交给真实 git 解包，并取出 HEAD 对象
    let unpack_dir = tempfile::tempdir().unwrap();
    git_ok(
        unpack_dir.path(),
        &["-c", "init.defaultBranch=main", "init", "-q", "."],
    );
    let mut child = Command::new("git")
        .args(["unpack-objects", "-q"])
        .current_dir(unpack_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&pack).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git unpack-objects 拒绝了从帧里重建的 pack: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ty = git_ok(unpack_dir.path(), &["cat-file", "-t", &head]);
    assert_eq!(
        String::from_utf8_lossy(&ty.stdout).trim(),
        "commit",
        "重建的 pack 里应含真实的 HEAD commit"
    );
}

// ---------------------------------------------------------------------------
// 第二个真实 git 数据源：v2 能力广告（http-backend）
// ---------------------------------------------------------------------------

#[test]
fn real_git_v2_capability_advertisement_parses() {
    let repo = init_repo_with_commit();
    let project_root = repo.path();
    let out = Command::new("git")
        .arg("http-backend")
        .env("GIT_PROTOCOL", "version=2")
        .env("REQUEST_METHOD", "GET")
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("GIT_PROJECT_ROOT", project_root)
        .env("PATH_INFO", "/.git/info/refs")
        .env("QUERY_STRING", "service=git-upload-pack")
        .current_dir(project_root)
        .output()
        .expect("V8 验证需要 git http-backend");
    assert!(out.status.success(), "git http-backend 失败");
    let split = out
        .stdout
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("CGI 响应必须有 header 分隔");
    let body = &out.stdout[split + 4..];
    assert!(!body.is_empty());

    let oracle = oracle_frames(body);
    assert_eq!(oracle.last(), Some(&RawFrame::Flush));
    let (pkts, spans) = read_all(body);
    assert_eq!(pkts.len(), oracle.len());
    assert_eq!(spans.last().unwrap().1, body.len());
    match &pkts[0] {
        Pkt::Data(d) => assert_eq!(d.as_slice(), b"version 2\n", "v2 广告首帧必须是 version 2"),
        other => panic!("首帧应为 Data，实际 {other:?}"),
    }
    assert!(
        pkts.iter()
            .any(|p| matches!(p, Pkt::Data(d) if d.windows(3).any(|w| w == b"git"))),
        "广告里应有 agent 之类的能力行"
    );
    for (i, pkt) in pkts.iter().enumerate() {
        match (pkt, &oracle[i]) {
            (Pkt::Data(d), RawFrame::Data(w)) => assert_eq!(d, w),
            (Pkt::Flush, RawFrame::Flush) => {}
            (a, b) => panic!("第 {i} 帧类型不符: {a:?} vs {b:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// (B)2 构造型边界（按规格自造，不用 encode_pkt）
// ---------------------------------------------------------------------------

#[test]
fn accepts_spec_boundary_frames_and_consumes_exactly_one_frame() {
    // 0004 = 合法空 Data
    let mut c = Cursor::new(&b"0004"[..]);
    assert_eq!(read_pkt(&mut c).unwrap(), Pkt::Data(Vec::new()));
    assert_eq!(c.position(), 4);

    // 0000 / 0001 / 0002：读到就返回，且只消费 4 字节
    let special: [(&[u8], Pkt); 3] = [
        (b"0000", Pkt::Flush),
        (b"0001", Pkt::Delim),
        (b"0002", Pkt::ResponseEnd),
    ];
    for (raw, want) in special {
        let mut c = Cursor::new(raw);
        assert_eq!(read_pkt(&mut c).unwrap(), want);
        assert_eq!(c.position(), 4, "特殊帧只应消费 4 字节");
    }

    // 最大 payload：65516（整帧 65520 = fff0）
    let payload: Vec<u8> = (0..MAX_PAYLOAD_LEN).map(|i| (i % 251) as u8).collect();
    let raw = spec_data(&payload);
    assert_eq!(&raw[..4], b"fff0");
    assert_eq!(raw.len(), MAX_PAYLOAD_LEN + 4);
    let mut c = Cursor::new(raw.as_slice());
    assert_eq!(read_pkt(&mut c).unwrap(), Pkt::Data(payload));
    assert_eq!(c.position() as usize, raw.len());

    // 连续多帧一次写入 → 逐帧读出，游标必须严格按帧长前进
    let want = [
        Pkt::Data(b"one".to_vec()),
        Pkt::Delim,
        Pkt::Data(b"two".to_vec()),
        Pkt::ResponseEnd,
        Pkt::Flush,
    ];
    let mut stream = Vec::new();
    let mut expected_positions = Vec::new();
    let mut off = 0usize;
    for pkt in &want {
        match pkt {
            Pkt::Data(d) => {
                stream.extend_from_slice(&spec_data(d));
                off += d.len() + 4;
            }
            Pkt::Flush => {
                stream.extend_from_slice(b"0000");
                off += 4;
            }
            Pkt::Delim => {
                stream.extend_from_slice(b"0001");
                off += 4;
            }
            Pkt::ResponseEnd => {
                stream.extend_from_slice(b"0002");
                off += 4;
            }
        }
        expected_positions.push(off as u64);
    }
    let mut c = Cursor::new(stream.as_slice());
    for (pkt, pos) in want.iter().zip(expected_positions.iter()) {
        assert_eq!(read_pkt(&mut c).unwrap(), *pkt);
        assert_eq!(c.position(), *pos, "第 N 帧读完后游标位置不对");
    }
}

#[test]
fn illegal_frames_are_protocol_errors_never_panics_or_partial_data() {
    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("长度 0003（<4）", b"0003".to_vec()),
        (
            "声明 fff1（65521 > 65520，字节齐全）",
            spec_data(&vec![0u8; 65517]),
        ),
        (
            "声明 ffff（65535 > 65520，字节齐全）",
            spec_data(&vec![0u8; 65531]),
        ),
        ("非十六进制 zzzz", b"zzzz".to_vec()),
        ("非十六进制 00g0", b"00g0".to_vec()),
        ("长度前缀含空格 ' 004'", b" 004".to_vec()),
        ("截断：空流（帧边界 EOF）", Vec::new()),
        ("截断：只有 2 字节", b"00".to_vec()),
        ("截断：只有 3 字节", b"000".to_vec()),
        ("截断：声明 0008 但无 payload", b"0008".to_vec()),
        ("截断：声明 0008 只给 2 字节", b"0008ab".to_vec()),
    ];
    let mut long = b"fff0".to_vec();
    long.extend_from_slice(b"abc");
    cases.push(("截断：声明最长帧 fff0 只给 3 字节", long));

    for (name, raw) in cases {
        let mut c = Cursor::new(raw.as_slice());
        match read_pkt(&mut c) {
            Err(Error::Protocol(_)) => {}
            other => panic!("{name}: 期望 Err(Error::Protocol)，实际 {other:?}"),
        }
        assert!((c.position() as usize) <= raw.len(), "{name}: 游标越过流尾");
    }
}

#[test]
fn side_band_channel_bytes_survive_verbatim() {
    for ch in [1u8, 2, 3] {
        let payload = [vec![ch], b"chunk".to_vec(), vec![0x00, 0xff, 0x01]].concat();
        let raw = spec_data(&payload);
        let mut c = Cursor::new(raw.as_slice());
        assert_eq!(read_pkt(&mut c).unwrap(), Pkt::Data(payload));
        assert_eq!(c.position() as usize, raw.len());
    }
    // 通道 0x01 出现在 payload 中间而非开头时同样原样保留
    let payload = b"progress: 50%\x01tail".to_vec();
    let raw = spec_data(&payload);
    let mut c = Cursor::new(raw.as_slice());
    assert_eq!(read_pkt(&mut c).unwrap(), Pkt::Data(payload));
}

// ---------------------------------------------------------------------------
// (B)3 写方向：字节形态、flush 调用、往返、超长拒绝
// ---------------------------------------------------------------------------

#[derive(Default)]
struct CountingWriter {
    buf: Vec<u8>,
    flushes: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

#[test]
fn write_direction_matches_spec_roundtrips_and_flushes() {
    // write_flush 输出就是 0000，并且真的调了 flush()
    let mut w = CountingWriter::default();
    write_flush(&mut w).unwrap();
    assert_eq!(w.buf, b"0000");
    assert!(w.flushes >= 1, "write_flush 必须 flush");

    // 写完后数据必须已 flush：用裸 Vec<u8> 断言（Vec 直接可见）
    let mut plain: Vec<u8> = Vec::new();
    write_pkt(&mut plain, b"hello\n").unwrap();
    assert_eq!(plain.as_slice(), b"000ahello\n");

    let payloads: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"a\n".to_vec(),
        b"NAK\n".to_vec(),
        [vec![2u8], vec![0x5au8; 997]].concat(),
        vec![0xffu8; MAX_PAYLOAD_LEN],
    ];
    for payload in payloads {
        let mut w = CountingWriter::default();
        let before = w.flushes;
        write_pkt(&mut w, &payload).unwrap();
        assert_eq!(
            w.buf,
            spec_data(&payload),
            "写出的字节不符合格式表（len({}) 字节）",
            payload.len()
        );
        assert!(w.flushes > before, "write_pkt 写完必须 flush()");
        if payload.len() == MAX_PAYLOAD_LEN {
            assert_eq!(&w.buf[..4], b"fff0");
        }
        // 往返：写出的字节被 read_pkt 读回必须相等
        let mut c = Cursor::new(w.buf.as_slice());
        assert_eq!(read_pkt(&mut c).unwrap(), Pkt::Data(payload.clone()));
        assert_eq!(c.position() as usize, w.buf.len(), "往返后必须正好读完");
    }

    // 超长 payload → Error::Protocol，且什么都不写、不 flush
    let mut w = CountingWriter::default();
    let too_big = vec![0u8; MAX_PAYLOAD_LEN + 1];
    match write_pkt(&mut w, &too_big) {
        Err(Error::Protocol(_)) => {}
        other => panic!("超长 data 应返回 Err(Error::Protocol)，实际 {other:?}"),
    }
    assert!(w.buf.is_empty(), "拒绝时必须不写任何字节");
    assert_eq!(w.flushes, 0, "拒绝时不应 flush");
}

// ---------------------------------------------------------------------------
// 防假绿：本文件自己也不靠 encode_pkt 提供真值 —— 这里自检 oracle 与规格构造器
// ---------------------------------------------------------------------------

#[test]
fn oracle_and_spec_builder_are_consistent_with_real_git() {
    // oracle 与 spec_data 都是本文件独立实现的；用真实 git 输出交叉验证一次
    let repo = init_repo_with_commit();
    let raw = advertise_refs_v0(&repo.path().join(".git"));
    let frames = oracle_frames(&raw);
    let rebuilt: Vec<u8> = frames
        .iter()
        .flat_map(|f| match f {
            RawFrame::Data(d) => spec_data(d),
            RawFrame::Flush => b"0000".to_vec(),
            RawFrame::Delim => b"0001".to_vec(),
            RawFrame::ResponseEnd => b"0002".to_vec(),
        })
        .collect();
    assert_eq!(rebuilt, raw, "oracle + spec_data 也必须能重建 git 的字节流");
}
