//! pkt-line 分帧。**T8（opencode）实现范围**。
//!
//! ```text
//! <4 位十六进制长度><payload>     # 长度包含这 4 个字节本身
//! 0000                            # flush-pkt（分段结束）
//! 0001                            # delim-pkt（v2 用）
//! 0002                            # response-end-pkt
//! ```
//!
//! 必须处理：长度 < 4 或 > 65520 时必须报 `Error::Protocol`；
//! 非十六进制字符；截断的流（EOF 在 payload 中间）；`side-band` 通道字节
//! （1=pack 数据，2=进度，3=错误）在 `Data` 里的原样保留。

use std::io::{BufRead, Write};

use crate::error::{Error, Result};

/// 单个 pkt 的最大 payload（git 限制总长 65520，含 4 字节长度前缀）。
pub const MAX_PAYLOAD_LEN: usize = 65516;

/// 单个 pkt 的最大帧长：4 字节长度前缀 + 最大 payload。
const MAX_FRAME_LEN: usize = MAX_PAYLOAD_LEN + 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pkt {
    Data(Vec<u8>),
    Flush,
    Delim,
    ResponseEnd,
}

/// 编码一个 pkt（纯 helper，测试构造 fixture 也用它）。
pub fn encode_pkt(data: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", data.len() + 4).into_bytes();
    out.extend_from_slice(data);
    out
}

pub fn read_pkt<R: BufRead>(reader: &mut R) -> Result<Pkt> {
    let mut len_buf = [0u8; 4];
    read_exact_protocol(reader, &mut len_buf, "pkt-line length prefix")?;

    let len = parse_len(&len_buf)?;
    match len {
        0 => return Ok(Pkt::Flush),
        1 => return Ok(Pkt::Delim),
        2 => return Ok(Pkt::ResponseEnd),
        _ => {}
    }

    if len < 4 {
        return Err(Error::Protocol(format!(
            "pkt-line length {len} is smaller than the 4-byte header"
        )));
    }
    if len > MAX_FRAME_LEN {
        return Err(Error::Protocol(format!(
            "pkt-line length {len} exceeds the {MAX_FRAME_LEN}-byte limit"
        )));
    }

    let mut payload = vec![0u8; len - 4];
    read_exact_protocol(reader, &mut payload, "pkt-line payload")?;
    Ok(Pkt::Data(payload))
}

pub fn write_pkt<W: Write>(writer: &mut W, data: &[u8]) -> Result<()> {
    if data.len() > MAX_PAYLOAD_LEN {
        return Err(Error::Protocol(format!(
            "pkt-line payload {} exceeds the {MAX_PAYLOAD_LEN}-byte limit",
            data.len()
        )));
    }
    writer.write_all(&encode_pkt(data))?;
    writer.flush()?;
    Ok(())
}

pub fn write_flush<W: Write>(writer: &mut W) -> Result<()> {
    writer.write_all(b"0000")?;
    writer.flush()?;
    Ok(())
}

fn parse_len(buf: &[u8; 4]) -> Result<usize> {
    let mut len = 0usize;
    for &byte in buf {
        let digit = match byte {
            b'0'..=b'9' => (byte - b'0') as usize,
            b'a'..=b'f' => (byte - b'a' + 10) as usize,
            b'A'..=b'F' => (byte - b'A' + 10) as usize,
            _ => {
                return Err(Error::Protocol(format!(
                    "invalid pkt-line length prefix (non-hex byte 0x{byte:02x})"
                )))
            }
        };
        len = len * 16 + digit;
    }
    Ok(len)
}

fn read_exact_protocol<R: BufRead>(reader: &mut R, buf: &mut [u8], what: &str) -> Result<()> {
    match reader.read_exact(buf) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Err(Error::Protocol(
            format!("truncated stream while reading {what}"),
        )),
        Err(err) => Err(Error::Io(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::process::{Command, Output};

    #[test]
    fn encodes_known_frames() {
        assert_eq!(encode_pkt(b"a\n"), b"0006a\n");
        assert_eq!(encode_pkt(b""), b"0004");
    }

    #[test]
    fn reads_special_frames_and_empty_data_without_extra_bytes() {
        let cases: [(&[u8], Pkt); 4] = [
            (b"0000", Pkt::Flush),
            (b"0001", Pkt::Delim),
            (b"0002", Pkt::ResponseEnd),
            (b"0004", Pkt::Data(Vec::new())),
        ];
        for (raw, want) in cases {
            let mut cursor = Cursor::new(raw);
            assert_eq!(read_pkt(&mut cursor).unwrap(), want);
            assert_eq!(cursor.position(), raw.len() as u64);
        }
    }

    #[test]
    fn reads_maximum_payload() {
        let payload = vec![0xab; MAX_PAYLOAD_LEN];
        let raw = encode_pkt(&payload);
        assert_eq!(&raw[..4], b"fff0");

        let mut cursor = Cursor::new(raw.as_slice());
        assert_eq!(read_pkt(&mut cursor).unwrap(), Pkt::Data(payload));
        assert_eq!(cursor.position(), MAX_FRAME_LEN as u64);
    }

    #[test]
    fn rejects_malformed_lengths() {
        for raw in [
            &b"0003"[..],
            &b"ffff"[..],
            &b"zzzz"[..],
            &b"00g0"[..],
            &b" 004"[..],
        ] {
            let mut cursor = Cursor::new(raw);
            let err = read_pkt(&mut cursor).unwrap_err();
            assert!(
                matches!(err, Error::Protocol(_)),
                "expected protocol error for {:?}, got {err:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn rejects_truncated_streams() {
        for raw in [
            &b""[..],
            &b"00"[..],
            &b"000"[..],
            &b"0008"[..],
            &b"0008ab"[..],
        ] {
            let mut cursor = Cursor::new(raw);
            let err = read_pkt(&mut cursor).unwrap_err();
            assert!(
                matches!(err, Error::Protocol(_)),
                "expected protocol error for {:?}, got {err:?}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn reads_consecutive_frames_and_tracks_cursor() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&encode_pkt(b"one"));
        raw.extend_from_slice(b"0001");
        raw.extend_from_slice(&encode_pkt(b"two"));
        raw.extend_from_slice(b"0002");
        raw.extend_from_slice(b"0000");

        let mut cursor = Cursor::new(raw.as_slice());
        assert_eq!(read_pkt(&mut cursor).unwrap(), Pkt::Data(b"one".to_vec()));
        assert_eq!(cursor.position(), 7);
        assert_eq!(read_pkt(&mut cursor).unwrap(), Pkt::Delim);
        assert_eq!(cursor.position(), 11);
        assert_eq!(read_pkt(&mut cursor).unwrap(), Pkt::Data(b"two".to_vec()));
        assert_eq!(cursor.position(), 18);
        assert_eq!(read_pkt(&mut cursor).unwrap(), Pkt::ResponseEnd);
        assert_eq!(cursor.position(), 22);
        assert_eq!(read_pkt(&mut cursor).unwrap(), Pkt::Flush);
        assert_eq!(cursor.position(), 26);
    }

    #[test]
    fn preserves_side_band_channel_bytes() {
        let payload = vec![2u8, b'p', b'r', b'o', b'g'];
        let raw = encode_pkt(&payload);
        let mut cursor = Cursor::new(raw.as_slice());
        assert_eq!(read_pkt(&mut cursor).unwrap(), Pkt::Data(payload));
    }

    #[test]
    fn write_roundtrips_through_read() {
        let mut buf = Vec::new();
        write_pkt(&mut buf, b"hello\n").unwrap();
        write_flush(&mut buf).unwrap();
        write_pkt(&mut buf, b"").unwrap();
        assert_eq!(buf.as_slice(), b"000ahello\n00000004");

        let mut cursor = Cursor::new(buf.as_slice());
        assert_eq!(
            read_pkt(&mut cursor).unwrap(),
            Pkt::Data(b"hello\n".to_vec())
        );
        assert_eq!(read_pkt(&mut cursor).unwrap(), Pkt::Flush);
        assert_eq!(read_pkt(&mut cursor).unwrap(), Pkt::Data(Vec::new()));
        assert_eq!(cursor.position(), buf.len() as u64);
    }

    #[test]
    fn write_pkt_rejects_oversized_payload() {
        let mut buf = Vec::new();
        let too_big = vec![0u8; MAX_PAYLOAD_LEN + 1];
        let err = write_pkt(&mut buf, &too_big).unwrap_err();
        assert!(matches!(err, Error::Protocol(_)));
        assert!(buf.is_empty());
    }

    fn git(args: &[&str], cwd: &std::path::Path) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("T8 differential tests require a real git binary on PATH")
    }

    fn repo_with_commit() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let out = git(&["init", "-q", "."], dir.path());
        assert!(
            out.status.success(),
            "git init: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        std::fs::write(dir.path().join("f.txt"), b"hello t8\n").unwrap();
        let out = git(&["add", "f.txt"], dir.path());
        assert!(
            out.status.success(),
            "git add: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = Command::new("git")
            .args(["commit", "-qm", "c1"])
            .current_dir(dir.path())
            .env("GIT_AUTHOR_NAME", "t8")
            .env("GIT_AUTHOR_EMAIL", "t8@example.com")
            .env("GIT_COMMITTER_NAME", "t8")
            .env("GIT_COMMITTER_EMAIL", "t8@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0800")
            .env("GIT_COMMITTER_DATE", "1700000000 +0800")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git commit: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        dir
    }

    fn advertise_refs(git_dir: &std::path::Path) -> Vec<u8> {
        let out = git(
            &[
                "-c",
                "protocol.version=0",
                "upload-pack",
                "--advertise-refs",
                ".",
            ],
            git_dir,
        );
        assert!(
            out.status.success(),
            "git upload-pack: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    #[test]
    fn differential_parses_and_reencodes_real_git_advertisement() {
        let dir = repo_with_commit();
        let raw = advertise_refs(&dir.path().join(".git"));
        assert!(!raw.is_empty());

        let mut cursor = Cursor::new(raw.as_slice());
        let mut frames = Vec::new();
        let mut rebuilt = Vec::new();
        loop {
            let start = cursor.position() as usize;
            let pkt = read_pkt(&mut cursor).expect("real git advertisement must parse");
            let end = cursor.position() as usize;
            match &pkt {
                Pkt::Data(data) => {
                    assert_eq!(
                        end - start,
                        data.len() + 4,
                        "read_pkt must consume exactly one frame"
                    );
                    let encoded = encode_pkt(data);
                    assert_eq!(
                        &raw[start..end],
                        encoded.as_slice(),
                        "round-trip byte mismatch"
                    );
                    rebuilt.extend_from_slice(&encoded);
                }
                Pkt::Flush => rebuilt.extend_from_slice(b"0000"),
                other => panic!("unexpected frame in v0 advertisement: {other:?}"),
            }
            let ended = matches!(pkt, Pkt::Flush);
            frames.push(pkt);
            if ended {
                break;
            }
        }

        assert_eq!(
            rebuilt, raw,
            "re-encoding every parsed frame must reproduce git's bytes"
        );
        assert!(
            matches!(frames.last(), Some(Pkt::Flush)),
            "advertisement must end with a flush-pkt"
        );

        let data_frames: Vec<&Vec<u8>> = frames
            .iter()
            .filter_map(|p| match p {
                Pkt::Data(d) => Some(d),
                _ => None,
            })
            .collect();
        assert!(
            data_frames.len() >= 2,
            "a committed repo advertises HEAD plus a branch"
        );
        let first = data_frames[0];
        assert!(first.len() > 45);
        assert!(first[..40].iter().all(u8::is_ascii_hexdigit));
        assert_eq!(&first[40..45], b" HEAD");
        assert_eq!(
            first[45], 0,
            "capabilities follow the NUL after the ref name"
        );
        assert!(
            data_frames
                .iter()
                .any(|d| d.windows(11).any(|w| w == b"refs/heads/")),
            "advertisement must list at least one branch ref"
        );
    }

    #[test]
    fn differential_feeds_written_request_to_real_git_upload_pack() {
        let dir = repo_with_commit();
        let git_dir = dir.path().join(".git");
        let adv_raw = advertise_refs(&git_dir);

        let mut adv_cursor = Cursor::new(adv_raw.as_slice());
        let oid = match read_pkt(&mut adv_cursor).unwrap() {
            Pkt::Data(d) => String::from_utf8(d[..40].to_vec()).unwrap(),
            other => panic!("unexpected first advertisement frame {other:?}"),
        };

        let mut body = Vec::new();
        write_pkt(&mut body, format!("want {oid} side-band-64k\n").as_bytes()).unwrap();
        write_flush(&mut body).unwrap();
        write_pkt(&mut body, b"done\n").unwrap();

        let mut body_cursor = Cursor::new(body.as_slice());
        assert!(matches!(read_pkt(&mut body_cursor).unwrap(), Pkt::Data(_)));
        assert_eq!(read_pkt(&mut body_cursor).unwrap(), Pkt::Flush);
        assert!(matches!(read_pkt(&mut body_cursor).unwrap(), Pkt::Data(_)));

        let mut child = Command::new("git")
            .args(["-c", "protocol.version=0", "upload-pack", "--stateless-rpc"])
            .arg(&git_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&body).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git rejected our written request: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.stdout.is_empty());

        let mut resp_cursor = Cursor::new(out.stdout.as_slice());
        let mut saw_band_channel = false;
        let mut data_frames = 0usize;
        loop {
            let pkt = read_pkt(&mut resp_cursor).expect("response must be valid pkt-line");
            match pkt {
                Pkt::Data(d) => {
                    data_frames += 1;
                    if data_frames == 1 {
                        assert!(
                            d.starts_with(b"NAK") || d.starts_with(b"ACK"),
                            "first response frame should be NAK/ACK, got {:?}",
                            String::from_utf8_lossy(&d)
                        );
                    }
                    if let Some(&channel) = d.first() {
                        if (1..=3).contains(&channel) {
                            saw_band_channel = true;
                        }
                    }
                }
                Pkt::Flush => break,
                other => panic!("unexpected response frame {other:?}"),
            }
        }
        assert!(data_frames >= 2);
        assert!(
            saw_band_channel,
            "side-band channel byte must be preserved verbatim in Data"
        );
    }
}
