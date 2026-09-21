//! `.pack` 解析。**T9（codex）实现范围**。
//!
//! ```text
//! "PACK" + version:u32be(=2 或 3) + count:u32be
//! 对象记录 × count
//! trailer: 前面所有字节的 sha1
//! ```
//!
//! 对象头（紧凑 varint）：
//! * 第 1 字节：bit7 续位，bit6-4 类型（1=commit 2=tree 3=blob 4=tag 6=ofs_delta 7=ref_delta），
//!   bit3-0 为 size 低 4 位。
//! * 后续字节：bit7 续位，bit6-0 为更高位（每字节 7 bit）。
//! * `OFS_DELTA`：紧接一个「负偏移」varint，编码为 `((v+1) << 7) | next` 形式——
//!   `byte = read(); offset = byte & 0x7f; while byte & 0x80 { byte = read(); offset = ((offset + 1) << 7) | (byte & 0x7f) }`。
//! * `REF_DELTA`：紧接 20 字节 base oid。
//! * 之后是 zlib stream（类型为 delta 时，解压出来的是 delta 数据而非对象内容）。
//!
//! **size 字段的语义**（用 `git verify-pack -v` 的真实 pack 对照确认过）：
//! 非 delta 对象 = 解压后的载荷长度；delta 对象 = 解压后的 **delta 数据**长度
//! （target 的长度由 delta 头里的 `target_size` 决定，不是这个字段）。
//!
//! `open` 会校验尾部 20 字节的全文件 sha1：截断/翻字节的 pack 在 `open` 就报 `Corrupt`，
//! 代价是每个 `PackFile` 一次 O(size) 哈希（`mg fsck` 之外的第二道闸门）。

use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::object::Kind;
use crate::oid::{Oid, RAW_LEN};
use crate::zlib;

use super::delta::apply_delta;
use super::idx::PackIndex;

const PACK_MAGIC: &[u8; 4] = b"PACK";
const PACK_HEADER_LEN: usize = 12;
const PACK_TRAILER_LEN: usize = RAW_LEN;
/// delta 链深度上限：真实 git 的 `--depth` 上限是 4095，这里给足余量同时防递归爆炸。
const MAX_DELTA_DEPTH: usize = 64;
/// 一条 delta 链上所有解压出来的字节总量上限（防压缩炸弹）。取 4 GiB：
/// 远超本项目规模的真实 pack，只有畸形的链才会撞上（撞上 = `Corrupt`，不是 panic）。
const MAX_CHAIN_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const TYPE_COMMIT: u8 = 1;
const TYPE_TREE: u8 = 2;
const TYPE_BLOB: u8 = 3;
const TYPE_TAG: u8 = 4;
const TYPE_OFS_DELTA: u8 = 6;
const TYPE_REF_DELTA: u8 = 7;

fn kind_from_type(ty: u8) -> Option<Kind> {
    match ty {
        TYPE_COMMIT => Some(Kind::Commit),
        TYPE_TREE => Some(Kind::Tree),
        TYPE_BLOB => Some(Kind::Blob),
        TYPE_TAG => Some(Kind::Tag),
        _ => None,
    }
}

/// 解析出来的对象头。`size` 的语义见模块文档。
enum ObjectHeader {
    Base {
        kind: Kind,
        size: u64,
        data_pos: usize,
    },
    OfsDelta {
        size: u64,
        base_offset: u64,
        data_pos: usize,
    },
    RefDelta {
        size: u64,
        base_oid: Oid,
        data_pos: usize,
    },
}

#[derive(Debug)]
pub struct PackFile {
    pub path: PathBuf,
    data: Vec<u8>,
    count: u32,
    /// 同名 `.idx`（存在才加载）：`REF_DELTA` 的 base 反查与 `oid_at` 都要用它。
    idx: Option<PackIndex>,
}

impl PackFile {
    pub fn open(path: &Path) -> Result<PackFile> {
        let data = std::fs::read(path).map_err(|err| {
            Error::Io(std::io::Error::new(
                err.kind(),
                format!("{}: {err}", path.display()),
            ))
        })?;
        PackFile::from_bytes(path, data)
    }

    /// pack 头里声明的对象数量。
    pub fn object_count(&self) -> u32 {
        self.count
    }

    fn from_bytes(path: &Path, data: Vec<u8>) -> Result<PackFile> {
        let what = path.display().to_string();
        if data.len() < PACK_HEADER_LEN + PACK_TRAILER_LEN {
            return Err(Error::corrupt(
                what.as_str(),
                format!("file is {} bytes, too short to be a pack", data.len()),
            ));
        }
        if !data.starts_with(PACK_MAGIC) {
            return Err(Error::corrupt(
                what.as_str(),
                "bad magic (expected \"PACK\")",
            ));
        }
        let version = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        if version != 2 && version != 3 {
            return Err(Error::corrupt(
                what.as_str(),
                format!("unsupported pack version {version} (expected 2 or 3)"),
            ));
        }
        let count = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        let mut hasher = Sha1::new();
        hasher.update(&data[..data.len() - PACK_TRAILER_LEN]);
        let mut sum = [0u8; PACK_TRAILER_LEN];
        sum.copy_from_slice(&hasher.finalize());
        if sum[..] != data[data.len() - PACK_TRAILER_LEN..] {
            return Err(Error::corrupt(
                what.as_str(),
                "trailing checksum does not match the pack contents",
            ));
        }

        let idx = {
            let idx_path = path.with_extension("idx");
            if idx_path.is_file() {
                Some(PackIndex::open(&idx_path)?)
            } else {
                None
            }
        };

        Ok(PackFile {
            path: path.to_path_buf(),
            data,
            count,
            idx,
        })
    }

    /// 按字节偏移读出一个对象（自动解开 delta 链），返回 `(kind, payload)`。
    pub fn read_at(&self, offset: u64) -> Result<(Kind, Vec<u8>)> {
        let mut budget = MAX_CHAIN_BYTES;
        self.read_at_depth(offset, 0, &mut budget)
    }

    /// 按偏移反查对象 id（用 `.idx` 反查，并重算内容哈希确认）。
    pub fn oid_at(&self, offset: u64) -> Result<Oid> {
        let idx = self.idx.as_ref().ok_or(Error::Unsupported(
            "cannot map a pack offset back to an object id without a .idx file",
        ))?;
        let oid = idx.oid_at_offset(offset).ok_or_else(|| {
            self.corrupt(format!(
                "no object listed in {} starts at pack offset {offset}",
                idx.label()
            ))
        })?;
        let (kind, payload) = self.read_at(offset)?;
        let actual = Oid::hash_object(kind.as_str(), &payload);
        if actual != oid {
            return Err(self.corrupt(format!(
                "object at pack offset {offset} hashes to {actual}, but {} maps offset {offset} to {oid}",
                idx.label()
            )));
        }
        Ok(oid)
    }

    fn read_at_depth(
        &self,
        offset: u64,
        depth: usize,
        budget: &mut u64,
    ) -> Result<(Kind, Vec<u8>)> {
        if depth > MAX_DELTA_DEPTH {
            return Err(self.corrupt(format!(
                "delta chain at offset {offset} is deeper than {MAX_DELTA_DEPTH} levels"
            )));
        }

        match self.parse_header(offset)? {
            ObjectHeader::Base {
                kind,
                size,
                data_pos,
            } => {
                let (payload, _) = self.inflate_at(data_pos)?;
                if payload.len() as u64 != size {
                    return Err(self.corrupt(format!(
                        "object at offset {offset}: header says {size} bytes but the payload is {} bytes",
                        payload.len()
                    )));
                }
                charge(budget, payload.len() as u64).map_err(|detail| self.corrupt(detail))?;
                Ok((kind, payload))
            }
            ObjectHeader::OfsDelta {
                size,
                base_offset,
                data_pos,
            } => self.read_delta(offset, size, base_offset, data_pos, depth, budget),
            ObjectHeader::RefDelta {
                size,
                base_oid,
                data_pos,
            } => {
                let idx = self.idx.as_ref().ok_or(Error::Unsupported(
                    "REF_DELTA in a pack without a .idx: the base object cannot be found by id",
                ))?;
                let base_offset = idx.lookup(base_oid).ok_or(Error::Unsupported(
                    "REF_DELTA whose base object is not in this pack (thin pack)",
                ))?;
                self.read_delta(offset, size, base_offset, data_pos, depth, budget)
            }
        }
    }

    fn read_delta(
        &self,
        offset: u64,
        size: u64,
        base_offset: u64,
        data_pos: usize,
        depth: usize,
        budget: &mut u64,
    ) -> Result<(Kind, Vec<u8>)> {
        let (delta, _) = self.inflate_at(data_pos)?;
        if delta.len() as u64 != size {
            return Err(self.corrupt(format!(
                "delta at offset {offset}: header says {size} bytes but the delta data is {} bytes",
                delta.len()
            )));
        }
        charge(budget, delta.len() as u64).map_err(|detail| self.corrupt(detail))?;
        let (kind, base) = self.read_at_depth(base_offset, depth + 1, budget)?;
        let payload = apply_delta(&base, &delta)?;
        charge(budget, payload.len() as u64).map_err(|detail| self.corrupt(detail))?;
        Ok((kind, payload))
    }

    /// 解析对象头：`(类型, size, zlib 流起点)` 或 delta 的 base 信息。
    fn parse_header(&self, offset: u64) -> Result<ObjectHeader> {
        let bound = self.object_bound();
        let start = usize::try_from(offset)
            .map_err(|_| self.corrupt(format!("offset {offset} does not fit in memory")))?;
        if start < PACK_HEADER_LEN {
            return Err(self.corrupt(format!("offset {offset} points into the pack header")));
        }
        if start >= bound {
            return Err(self.corrupt(format!(
                "offset {offset} is past the last object (pack data ends at {bound})"
            )));
        }

        let mut pos = start;
        let mut byte = self.byte_at(pos, bound)?;
        pos += 1;
        let ty = (byte >> 4) & 0x07;
        let mut size = u64::from(byte & 0x0f);
        let mut shift = 4u32;
        while byte & 0x80 != 0 {
            byte = self.byte_at(pos, bound)?;
            pos += 1;
            if shift >= u64::BITS {
                return Err(self.corrupt(format!(
                    "object header at offset {offset} has a size wider than 64 bits"
                )));
            }
            size |= u64::from(byte & 0x7f) << shift;
            shift += 7;
        }

        match ty {
            TYPE_OFS_DELTA => {
                let mut byte = self.byte_at(pos, bound)?;
                pos += 1;
                let mut distance = u64::from(byte & 0x7f);
                while byte & 0x80 != 0 {
                    byte = self.byte_at(pos, bound)?;
                    pos += 1;
                    // 与 git 的 `ofs_delta` 编码一致：`((v + 1) << 7) | next`。
                    distance = distance
                        .checked_add(1)
                        .and_then(|value| value.checked_mul(128))
                        .and_then(|value| value.checked_add(u64::from(byte & 0x7f)))
                        .ok_or_else(|| {
                            self.corrupt(format!(
                                "ofs_delta at offset {offset}: base distance overflows"
                            ))
                        })?;
                }
                if distance == 0 {
                    return Err(self.corrupt(format!(
                        "ofs_delta at offset {offset} points at itself (zero base distance)"
                    )));
                }
                let base_offset = offset.checked_sub(distance).ok_or_else(|| {
                    self.corrupt(format!(
                        "ofs_delta at offset {offset}: base offset underflows (distance {distance})"
                    ))
                })?;
                if base_offset < PACK_HEADER_LEN as u64 {
                    return Err(self.corrupt(format!(
                        "ofs_delta at offset {offset} points into the pack header (base {base_offset})"
                    )));
                }
                Ok(ObjectHeader::OfsDelta {
                    size,
                    base_offset,
                    data_pos: pos,
                })
            }
            TYPE_REF_DELTA => {
                let end = pos.checked_add(RAW_LEN).ok_or_else(|| {
                    self.corrupt(format!("ref_delta at offset {offset}: header overflows"))
                })?;
                if end > bound {
                    return Err(self.corrupt(format!(
                        "ref_delta at offset {offset}: base oid runs past the end of the pack data"
                    )));
                }
                let base_oid = Oid::from_slice(&self.data[pos..end])
                    .map_err(|err| self.corrupt(format!("ref_delta at offset {offset}: {err}")))?;
                Ok(ObjectHeader::RefDelta {
                    size,
                    base_oid,
                    data_pos: end,
                })
            }
            other => match kind_from_type(other) {
                Some(kind) => Ok(ObjectHeader::Base {
                    kind,
                    size,
                    data_pos: pos,
                }),
                None => Err(self.corrupt(format!(
                    "object at offset {offset} has type {other} (reserved)"
                ))),
            },
        }
    }

    /// 从 `pos` 解压一个 zlib stream，返回 `(解压结果, 消耗的压缩字节数)`。
    fn inflate_at(&self, pos: usize) -> Result<(Vec<u8>, usize)> {
        let bound = self.object_bound();
        if pos >= bound {
            return Err(self.corrupt(format!(
                "no zlib stream at offset {pos}: pack data ends at {bound}"
            )));
        }
        let (out, consumed) = zlib::inflate_prefix(&self.data[pos..bound])
            .map_err(|err| self.corrupt(format!("object data at offset {pos}: {err}")))?;
        if pos + consumed > bound {
            return Err(self.corrupt(format!(
                "zlib stream at offset {pos} runs into the pack trailer"
            )));
        }
        Ok((out, consumed))
    }

    fn object_bound(&self) -> usize {
        self.data.len() - PACK_TRAILER_LEN
    }

    fn byte_at(&self, pos: usize, bound: usize) -> Result<u8> {
        if pos >= bound {
            return Err(self.corrupt(format!(
                "object header runs past the end of the pack data (offset {pos}, limit {bound})"
            )));
        }
        Ok(self.data[pos])
    }

    fn corrupt(&self, detail: impl Into<String>) -> Error {
        Error::corrupt(self.path.display().to_string(), detail)
    }
}

/// delta 链的总字节预算（失败返回 `Corrupt` 的 detail）。
fn charge(budget: &mut u64, bytes: u64) -> std::result::Result<(), String> {
    if bytes > *budget {
        return Err(format!(
            "delta chain decompressed more than {MAX_CHAIN_BYTES} bytes"
        ));
    }
    *budget -= bytes;
    Ok(())
}

// ---------------------------------------------------------------------------
// 真 git 测试台。真值只来自 `git` 进程与文件系统；这里没有任何 hardcode 的 oid
// 或期望字节（`scan_pack` 只读结构，用于「这个 pack 里确实有 OFS_DELTA」这类断言）。
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod gitkit {
    #![allow(dead_code)]

    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};

    use sha1::{Digest, Sha1};

    use crate::oid::Oid;
    use crate::zlib;

    pub fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// 与真 git 的隔离调用：配置/身份/日期都只作用于这条命令，绝不 export 到共享 shell。
    pub fn git_command(dir: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "T9")
            .env("GIT_AUTHOR_EMAIL", "t9@example.com")
            .env("GIT_COMMITTER_NAME", "T9")
            .env("GIT_COMMITTER_EMAIL", "t9@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0800")
            .env("GIT_COMMITTER_DATE", "1700000000 +0800")
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY");
        for global in [
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.autocrlf=false",
            "-c",
            "commit.gpgsign=false",
        ] {
            cmd.arg(global);
        }
        cmd.args(args);
        cmd
    }

    pub fn git_raw(dir: &Path, args: &[&str]) -> Output {
        git_command(dir, args)
            .output()
            .expect("failed to spawn git")
    }

    pub fn git(dir: &Path, args: &[&str]) -> String {
        let out = git_raw(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    pub fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
        let out = git_raw(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    pub fn git_stdin(dir: &Path, args: &[&str], input: &[u8]) -> Vec<u8> {
        let mut cmd = git_command(dir, args);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("failed to spawn git");
        child.stdin.as_mut().unwrap().write_all(input).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    /// 一个有内容的真实仓库：文本（大量相似版本 → 逼 git 用 delta）、二进制、
    /// 空文件、大文件，两次提交。
    pub fn make_fixture_repo(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();

        let base: Vec<String> = (0..80)
            .map(|i| {
                format!("line {i}: the quick brown fox jumps over the lazy dog and runs away\n")
            })
            .collect();
        for variant in 0..24usize {
            let mut lines = base.clone();
            for edit in 0..(variant % 8 + 1) {
                let idx = (variant * 7 + edit * 13) % lines.len();
                lines[idx] = format!("line {idx}: VARIANT {variant} changed this line\n");
            }
            std::fs::write(data.join(format!("v{variant:02}.txt")), lines.concat()).unwrap();
        }

        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/empty"), b"").unwrap();
        std::fs::write(dir.join("bin/repeat.bin"), b"ABCDEFGH".repeat(30_000)).unwrap();
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut random = vec![0u8; 200_000];
        for byte in random.iter_mut() {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            *byte = (state >> 33) as u8;
        }
        std::fs::write(dir.join("bin/random.bin"), random).unwrap();

        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "fixture one"]);

        // 第二轮：同一批文件的小改动 → 多级 delta 链。
        let mut updated = std::fs::read_to_string(data.join("v00.txt")).unwrap();
        updated.push_str("a trailing line added by the second commit\n");
        std::fs::write(data.join("v00.txt"), updated).unwrap();
        std::fs::copy(data.join("v01.txt"), data.join("v01-copy.txt")).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "fixture two"]);
    }

    /// `objects/pack/*.idx`（升序）。
    pub fn pack_files(dir: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let pack_dir = dir.join(".git/objects/pack");
        for entry in std::fs::read_dir(&pack_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "idx") {
                found.push(path);
            }
        }
        found.sort();
        found
    }

    /// `git cat-file --batch-all-objects --batch-check` → `(oid, type, size)`，按 oid 排序。
    pub fn git_objects(dir: &Path) -> Vec<(Oid, String, u64)> {
        let out = git(
            dir,
            &[
                "cat-file",
                "--batch-all-objects",
                "--batch-check=%(objectname) %(objecttype) %(objectsize)",
            ],
        );
        let mut objects = Vec::new();
        for line in out.lines() {
            let mut fields = line.split_whitespace();
            let oid = fields.next().expect("oid field");
            let kind = fields.next().expect("type field").to_string();
            let size: u64 = fields.next().expect("size field").parse().unwrap();
            objects.push((Oid::from_hex(oid).unwrap(), kind, size));
        }
        objects.sort();
        assert!(!objects.is_empty(), "fixture repo has no objects");
        objects
    }

    pub struct VerifyEntry {
        pub oid: String,
        pub kind: String,
        /// 注意：对 delta 对象来说这是 **delta 数据**的长度（git 的头字段语义）。
        pub size: u64,
        pub size_in_pack: u64,
        pub offset: u64,
        /// 0 = 非 delta；>= 2 = 多级链。
        pub depth: usize,
        pub base: Option<String>,
    }

    /// 解析 `git verify-pack -v <idx>` 的对象行。
    pub fn verify_pack(dir: &Path, idx: &Path) -> Vec<VerifyEntry> {
        let out = git(dir, &["verify-pack", "-v", &idx.display().to_string()]);
        let mut entries = Vec::new();
        for line in out.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 5
                || fields[0].len() != 40
                || !fields[0].chars().all(|c| c.is_ascii_hexdigit())
            {
                continue; // 汇总行（"non delta:" / "chain length ="）与结尾的 "<idx>: ok"
            }
            entries.push(VerifyEntry {
                oid: fields[0].to_string(),
                kind: fields[1].to_string(),
                size: fields[2].parse().unwrap(),
                size_in_pack: fields[3].parse().unwrap(),
                offset: fields[4].parse().unwrap(),
                depth: fields.get(5).map(|d| d.parse().unwrap()).unwrap_or(0),
                base: fields.get(6).map(|base| (*base).to_string()),
            });
        }
        assert!(
            !entries.is_empty(),
            "{idx:?}: verify-pack reported no objects"
        );
        entries
    }

    pub struct RawObject {
        pub offset: u64,
        pub ty: u8,
        pub size: u64,
        pub data_pos: usize,
        pub compressed_len: usize,
    }

    /// 测试自带的 pack 扫描器（**与被测实现无关**）：只用来回答
    /// 「这个 pack 里有没有 OFS_DELTA / 某个对象的 zlib 流在哪儿」。
    pub fn scan_pack(path: &Path) -> Vec<RawObject> {
        let data = std::fs::read(path).unwrap();
        assert!(data.starts_with(b"PACK"), "{path:?} is not a pack");
        let count = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        let bound = data.len() - 20;
        let mut objects = Vec::new();
        let mut pos = 12usize;
        for _ in 0..count {
            let offset = pos as u64;
            let mut byte = data[pos];
            pos += 1;
            let ty = (byte >> 4) & 0x07;
            let mut size = u64::from(byte & 0x0f);
            let mut shift = 4u32;
            while byte & 0x80 != 0 {
                byte = data[pos];
                pos += 1;
                size |= u64::from(byte & 0x7f) << shift;
                shift += 7;
            }
            if ty == 6 {
                let mut byte = data[pos];
                pos += 1;
                while byte & 0x80 != 0 {
                    byte = data[pos];
                    pos += 1;
                }
            } else if ty == 7 {
                pos += 20;
            }
            let (_, consumed) = zlib::inflate_prefix(&data[pos..bound]).unwrap();
            objects.push(RawObject {
                offset,
                ty,
                size,
                data_pos: pos,
                compressed_len: consumed,
            });
            pos += consumed;
        }
        assert_eq!(pos, bound, "scan did not consume the whole pack body");
        objects
    }

    /// 重算 pack 尾部的全文件 sha1（故意改坏 pack 字节后用，好让测试打到结构校验而不是校验和）。
    pub fn resign_pack(bytes: &mut [u8]) {
        let len = bytes.len();
        let mut hasher = Sha1::new();
        hasher.update(&bytes[..len - 20]);
        let sum = hasher.finalize();
        bytes[len - 20..].copy_from_slice(&sum);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::gitkit;
    use super::*;

    fn fixture_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        gitkit::make_fixture_repo(tmp.path());
        tmp
    }

    fn write_bytes(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    /// 验收 1/3：真实 `git repack -adf --window=50 --depth=50` 产出的 pack，
    /// 每个对象都与 `git cat-file <type> <oid>` 逐字节一致；idx 的偏移与 verify-pack 一致。
    #[test]
    fn reads_every_object_of_a_git_repack_byte_for_byte() {
        if !gitkit::git_available() {
            eprintln!("git is not available: skipping");
            return;
        }
        let tmp = fixture_dir();
        let dir = tmp.path();
        gitkit::git(dir, &["repack", "-adf", "--window=50", "--depth=50"]);

        let packs = gitkit::pack_files(dir);
        assert_eq!(packs.len(), 1, "expected exactly one pack, got {packs:?}");
        let pack_path = packs[0].with_extension("pack");

        let idx = PackIndex::open(&packs[0]).unwrap();
        let pack = PackFile::open(&pack_path).unwrap();
        let objects = gitkit::git_objects(dir);
        assert!(
            objects.len() >= 30,
            "fixture is too small: {} objects",
            objects.len()
        );
        assert_eq!(usize::try_from(pack.object_count()).unwrap(), objects.len());

        let raw = gitkit::scan_pack(&pack_path);
        assert_eq!(raw.len(), objects.len());
        assert!(
            raw.iter().any(|object| object.ty == 6),
            "fixture produced no OFS_DELTA object"
        );
        assert!(
            !raw.iter().any(|object| object.ty == 7),
            "repack should emit OFS_DELTA, not REF_DELTA"
        );

        for (oid, kind_name, size) in &objects {
            let offset = idx
                .lookup(*oid)
                .unwrap_or_else(|| panic!("{oid} is missing from the index"));
            assert!(
                raw.iter().any(|object| object.offset == offset),
                "{oid}: offset {offset} is not an object start"
            );
            let (kind, payload) = pack
                .read_at(offset)
                .unwrap_or_else(|err| panic!("{oid} at offset {offset}: {err}"));
            assert_eq!(kind.as_str(), kind_name, "{oid}");
            assert_eq!(payload.len() as u64, *size, "{oid}");
            let expected = gitkit::git_bytes(dir, &["cat-file", kind_name, &oid.to_hex()]);
            assert_eq!(
                payload, expected,
                "{oid} ({kind_name}) differs from git cat-file"
            );
            assert_eq!(Oid::hash_object(kind_name, &payload), *oid, "{oid}");
            assert_eq!(pack.oid_at(offset).unwrap(), *oid);
        }

        // 验收 3：idx 反查出来的偏移与 `git verify-pack -v` 完全一致。
        let entries = gitkit::verify_pack(dir, &packs[0]);
        assert_eq!(entries.len(), objects.len());
        for entry in &entries {
            let oid = Oid::from_hex(&entry.oid).unwrap();
            assert_eq!(
                idx.lookup(oid),
                Some(entry.offset),
                "{}: idx offset differs from git verify-pack",
                entry.oid
            );
        }

        // 验收 1：至少一条多级 OFS_DELTA 链，逐个对拍（depth >= 2 就是多级链）。
        let deep: Vec<&gitkit::VerifyEntry> =
            entries.iter().filter(|entry| entry.depth >= 2).collect();
        assert!(
            !deep.is_empty(),
            "fixture produced no multi-level delta chain"
        );
        for entry in &deep {
            let ty = raw
                .iter()
                .find(|object| object.offset == entry.offset)
                .map(|object| object.ty);
            assert_eq!(
                ty,
                Some(6),
                "{}: a multi-level chain must be OFS_DELTA",
                entry.oid
            );
            let (kind, payload) = pack.read_at(entry.offset).unwrap();
            assert_eq!(kind.as_str(), entry.kind);
            assert_eq!(
                payload,
                gitkit::git_bytes(dir, &["cat-file", &entry.kind, &entry.oid])
            );
            assert_eq!(Oid::hash_object(&entry.kind, &payload).to_hex(), entry.oid);
        }
    }

    /// 验收 1/2：`git pack-objects`（不带 `--delta-base-offset`）产出的 REF_DELTA pack，
    /// 有 `.idx` 时全部可读；把 `.idx` 拿掉后 REF_DELTA 必须报 `Unsupported`（不能瞎猜）。
    #[test]
    fn reads_ref_deltas_with_an_index_and_reports_unsupported_without_one() {
        if !gitkit::git_available() {
            eprintln!("git is not available: skipping");
            return;
        }
        let tmp = fixture_dir();
        let dir = tmp.path();
        let revs = gitkit::git_bytes(dir, &["rev-list", "--objects", "--all"]);
        let pack_bytes = gitkit::git_stdin(dir, &["pack-objects", "--stdout"], &revs);
        // `git index-pack --stdin` 会把它放进 objects/pack/ 并写好 `.idx`。
        gitkit::git_stdin(dir, &["index-pack", "--stdin"], &pack_bytes);

        let ref_idx = gitkit::pack_files(dir)
            .into_iter()
            .find(|path| {
                gitkit::scan_pack(&path.with_extension("pack"))
                    .iter()
                    .any(|object| object.ty == 7)
            })
            .expect("`git pack-objects` did not produce REF_DELTA objects");
        let ref_pack_path = ref_idx.with_extension("pack");

        let idx = PackIndex::open(&ref_idx).unwrap();
        let pack = PackFile::open(&ref_pack_path).unwrap();
        let objects = gitkit::git_objects(dir);
        assert_eq!(usize::try_from(pack.object_count()).unwrap(), objects.len());
        for (oid, kind_name, size) in &objects {
            let offset = idx.lookup(*oid).unwrap();
            let (kind, payload) = pack
                .read_at(offset)
                .unwrap_or_else(|err| panic!("{oid}: {err}"));
            assert_eq!(kind.as_str(), kind_name, "{oid}");
            assert_eq!(payload.len() as u64, *size, "{oid}");
            assert_eq!(
                payload,
                gitkit::git_bytes(dir, &["cat-file", kind_name, &oid.to_hex()]),
                "{oid} ({kind_name}) differs from git cat-file"
            );
        }

        // 没有 `.idx`：REF_DELTA 报 Unsupported，非 delta 对象照样能读，oid_at 报 Unsupported。
        let copy_dir = tempfile::tempdir().unwrap();
        let copy = copy_dir.path().join("pack-no-idx.pack");
        fs::copy(&ref_pack_path, &copy).unwrap();
        let bare = PackFile::open(&copy).unwrap();
        let scan = gitkit::scan_pack(&copy);
        assert_eq!(usize::try_from(bare.object_count()).unwrap(), scan.len());

        let ref_delta = scan
            .iter()
            .find(|object| object.ty == 7)
            .expect("scan found no REF_DELTA");
        let err = bare.read_at(ref_delta.offset).unwrap_err();
        assert!(
            matches!(err, Error::Unsupported(_)),
            "REF_DELTA without an index must be Unsupported, got {err:?}"
        );
        assert!(matches!(
            bare.oid_at(ref_delta.offset),
            Err(Error::Unsupported(_))
        ));

        let stored = scan
            .iter()
            .find(|object| (1..=4).contains(&object.ty))
            .expect("scan found no stored object");
        let (kind, payload) = bare.read_at(stored.offset).unwrap();
        assert!((1..=4).contains(&stored.ty));
        assert_eq!(payload.len() as u64, stored.size);
        assert!(matches!(
            kind,
            Kind::Blob | Kind::Tree | Kind::Commit | Kind::Tag
        ));
    }

    /// 验收 4：截断/篡改/magic/version/保留类型/zlib 流损坏/delta copy 越界/自引用 delta
    /// 都必须 `Err(Corrupt)`（或 `Unsupported`），不得 panic、不得返回部分数据。
    #[test]
    fn rejects_corrupt_packs_instead_of_returning_partial_data() {
        if !gitkit::git_available() {
            eprintln!("git is not available: skipping");
            return;
        }
        let tmp = fixture_dir();
        let dir = tmp.path();
        gitkit::git(dir, &["repack", "-adf"]);
        let packs = gitkit::pack_files(dir);
        let good_path = packs[0].with_extension("pack");
        let good = fs::read(&good_path).unwrap();
        let scan = gitkit::scan_pack(&good_path);

        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path();

        // (a) 截断 / 太短 / 根本不是 pack。
        for (name, bytes) in [
            ("half.pack", good[..good.len() / 2].to_vec()),
            ("tiny.pack", good[..8].to_vec()),
            ("garbage.pack", b"definitely not a packfile at all".to_vec()),
        ] {
            let path = write_bytes(out, name, &bytes);
            let err = PackFile::open(&path).unwrap_err();
            assert!(
                matches!(err, Error::Corrupt { .. }),
                "{name}: expected Corrupt, got {err:?}"
            );
        }

        // (b) magic / version 被改（校验和自洽，只有结构检查能抓到）。
        let mut bad = good.clone();
        bad[0] = b'X';
        let path = write_bytes(out, "bad-magic.pack", &bad);
        assert!(matches!(
            PackFile::open(&path).unwrap_err(),
            Error::Corrupt { .. }
        ));

        let mut bad = good.clone();
        bad[4..8].copy_from_slice(&4u32.to_be_bytes());
        gitkit::resign_pack(&mut bad);
        let path = write_bytes(out, "bad-version.pack", &bad);
        assert!(matches!(
            PackFile::open(&path).unwrap_err(),
            Error::Corrupt { .. }
        ));

        // (c) 尾部 sha1 被改坏（不重算）→ open 就报 Corrupt。
        let mut bad = good.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        let path = write_bytes(out, "bad-checksum.pack", &bad);
        assert!(matches!(
            PackFile::open(&path).unwrap_err(),
            Error::Corrupt { .. }
        ));

        // (d) 某个对象的 zlib 流被破坏（重算 sha1）→ open 成功，read_at 报 Corrupt。
        let stored = scan
            .iter()
            .find(|object| (1..=4).contains(&object.ty))
            .expect("scan found no stored object");
        let mut bad = good.clone();
        bad[stored.data_pos + stored.compressed_len / 2] ^= 0xff;
        gitkit::resign_pack(&mut bad);
        let path = write_bytes(out, "broken-zlib.pack", &bad);
        let pack = PackFile::open(&path).unwrap();
        let err = pack.read_at(stored.offset).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "expected Corrupt, got {err:?}"
        );

        // (e) 保留类型 5 → Corrupt。
        let mut bad = good.clone();
        let ty_pos = stored.offset as usize;
        bad[ty_pos] = (5 << 4) | (bad[ty_pos] & 0x0f);
        gitkit::resign_pack(&mut bad);
        let path = write_bytes(out, "reserved-type.pack", &bad);
        let pack = PackFile::open(&path).unwrap();
        let err = pack.read_at(stored.offset).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "expected Corrupt, got {err:?}"
        );

        // (f) delta 指令里 copy 越界：保留真实 delta 的两个 varint，把指令流换成
        //     一条 offset 必然越界的 copy，再重算 sha1。
        let delta_object = scan
            .iter()
            .find(|object| object.ty == 6)
            .expect("scan found no OFS_DELTA");
        let (delta, _) =
            zlib::inflate_prefix(&good[delta_object.data_pos..good.len() - 20]).unwrap();
        let mut header_len = 0usize;
        for _ in 0..2 {
            loop {
                let byte = delta[header_len];
                header_len += 1;
                if byte & 0x80 == 0 {
                    break;
                }
            }
        }
        let mut evil = delta[..header_len].to_vec();
        // 0xff = copy：4 个 offset 字节 + 3 个 size 字节 → offset 0x7fffffff 必然越界。
        evil.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x7f, 0x00, 0x00, 0x00]);
        let deflated = zlib::deflate(&evil).unwrap();
        let mut bad = Vec::new();
        bad.extend_from_slice(&good[..delta_object.data_pos]);
        bad.extend_from_slice(&deflated);
        bad.extend_from_slice(&good[delta_object.data_pos + delta_object.compressed_len..]);
        gitkit::resign_pack(&mut bad);
        let path = write_bytes(out, "delta-copy-oob.pack", &bad);
        let pack = PackFile::open(&path).unwrap();
        let err = pack.read_at(delta_object.offset).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "expected Corrupt, got {err:?}"
        );

        // (g) 自引用的 OFS_DELTA（base 距离 = 0）→ Corrupt，而不是死循环/爆栈。
        let mut self_ref = Vec::new();
        self_ref.extend_from_slice(b"PACK");
        self_ref.extend_from_slice(&2u32.to_be_bytes());
        self_ref.extend_from_slice(&1u32.to_be_bytes());
        self_ref.push((6 << 4) | 15); // type 6 (ofs_delta), size 15
        self_ref.push(0x00); // 负偏移 varint = 0 → base 指向自己
        let dummy: Vec<u8> = (0..15u8).collect();
        self_ref.extend_from_slice(&zlib::deflate(&dummy).unwrap());
        gitkit::resign_pack(&mut self_ref);
        let path = write_bytes(out, "self-referencing.pack", &self_ref);
        let pack = PackFile::open(&path).unwrap();
        let err = pack.read_at(12).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "expected Corrupt, got {err:?}"
        );
    }
}
