//! `.idx` v2 读写。**T9（codex）实现范围**。
//!
//! ```text
//! "\377tOc" + version:u32be(=2)
//! fanout[256]: u32be            # 前 i 字节 oid 的累计数量
//! oid[fanout[255]]: 20 bytes    # 升序
//! crc32[fanout[255]]: u32be
//! offset[fanout[255]]: u32be    # 高位为 1 时表示大 offset 表的索引
//! big_offset[...]: u64be
//! trailer: pack 的 sha1 + idx 自身的 sha1
//! ```
//!
//! `open` 是「严格读」：magic / version / fanout 单调性 / oid 升序 / 长度自洽 / 尾部 sha1
//! 任何一项不满足都是 `Error::Corrupt`，绝不返回部分数据（否则会退化成
//! 「对象查不到 = ObjectNotFound」的假绿）。**不**逐项校验 crc32——那是 `mg fsck` 的活。

use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::oid::{Oid, RAW_LEN};

const IDX_MAGIC: &[u8; 4] = b"\xfftOc";
const IDX_VERSION: u32 = 2;
const HEADER_LEN: usize = 8;
const FANOUT_ENTRIES: usize = 256;
const FANOUT_LEN: usize = FANOUT_ENTRIES * 4;
const ENTRY_LEN: usize = RAW_LEN + 4 + 4;
/// 尾部两个 sha1：pack 的 + idx 自身的。
const TRAILER_LEN: usize = 2 * RAW_LEN;
/// 最后 20 字节是 idx 自身的 sha1，覆盖它之前的所有内容。
const IDX_CHECKSUM_LEN: usize = RAW_LEN;
/// offset 表里这一位为 1 时，低 31 位是大 offset 表的索引。
const BIG_OFFSET_FLAG: u32 = 0x8000_0000;
/// 大 offset 表里的值必须真的超过 32 位能表示的范围（git 只在 offset >= 2^31 时才用它）。
const BIG_OFFSET_MIN: u64 = 0x8000_0000;

#[derive(Debug, Default, Clone)]
pub struct PackIndex {
    pub path: Option<std::path::PathBuf>,
    pub oids: Vec<Oid>,
    pub offsets: Vec<u64>,
}

impl PackIndex {
    pub fn open(path: &Path) -> Result<PackIndex> {
        let data = std::fs::read(path).map_err(|err| {
            Error::Io(std::io::Error::new(
                err.kind(),
                format!("{}: {err}", path.display()),
            ))
        })?;
        let (oids, offsets) = parse(&data).map_err(|err| match err {
            Error::Corrupt { what, detail } => {
                Error::corrupt(path.display().to_string(), format!("{what}: {detail}"))
            }
            other => other,
        })?;
        Ok(PackIndex {
            path: Some(path.to_path_buf()),
            oids,
            offsets,
        })
    }

    pub fn len(&self) -> usize {
        self.oids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.oids.is_empty()
    }

    /// 二分查找 oid 在 pack 中的字节偏移。
    pub fn lookup(&self, oid: Oid) -> Option<u64> {
        self.oids
            .binary_search(&oid)
            .ok()
            .map(|idx| self.offsets[idx])
    }

    pub fn iter_oids(&self) -> &[Oid] {
        &self.oids
    }

    /// 与本索引同名的 `.pack` 路径（扩展名替换）；没有来源路径时为 `None`。
    pub(crate) fn pack_path(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|path| path.with_extension("pack"))
    }

    /// 反查：返回「在 pack 中的字节偏移恰好是 `offset`」的对象 id。
    pub(crate) fn oid_at_offset(&self, offset: u64) -> Option<Oid> {
        self.oids
            .iter()
            .zip(&self.offsets)
            .find(|(_, &candidate)| candidate == offset)
            .map(|(oid, _)| *oid)
    }

    /// 供错误信息使用的人可读名字。
    pub(crate) fn label(&self) -> String {
        match &self.path {
            Some(path) => path.display().to_string(),
            None => "<in-memory pack index>".to_string(),
        }
    }
}

fn corrupt(detail: impl Into<String>) -> Error {
    Error::corrupt("pack index", detail)
}

fn u32be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn u64be(bytes: &[u8]) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[..8]);
    u64::from_be_bytes(raw)
}

/// 解析 `.idx` v2，返回 `(oids, offsets)`（两者一一对应且 oids 升序）。
fn parse(data: &[u8]) -> Result<(Vec<Oid>, Vec<u64>)> {
    if data.len() < HEADER_LEN + FANOUT_LEN + TRAILER_LEN {
        return Err(corrupt(format!(
            "file is {} bytes, too short for a v2 index header",
            data.len()
        )));
    }
    if !data.starts_with(IDX_MAGIC) {
        return Err(corrupt("bad magic (expected \"\\377tOc\")"));
    }
    let version = u32be(&data[4..8]);
    if version != IDX_VERSION {
        return Err(corrupt(format!(
            "unsupported index version {version} (expected {IDX_VERSION})"
        )));
    }

    let mut hasher = Sha1::new();
    hasher.update(&data[..data.len() - IDX_CHECKSUM_LEN]);
    let mut sum = [0u8; RAW_LEN];
    sum.copy_from_slice(&hasher.finalize());
    if sum[..] != data[data.len() - IDX_CHECKSUM_LEN..] {
        return Err(corrupt(
            "trailing checksum does not match the index contents",
        ));
    }

    let mut fanout = [0u32; FANOUT_ENTRIES];
    let mut previous = 0u32;
    for (i, slot) in fanout.iter_mut().enumerate() {
        let value = u32be(&data[HEADER_LEN + i * 4..HEADER_LEN + i * 4 + 4]);
        if value < previous {
            return Err(corrupt(format!(
                "fanout[{i}] = {value} is smaller than fanout[{}] = {previous}: not monotonic",
                i - 1
            )));
        }
        *slot = value;
        previous = value;
    }

    let n = fanout[FANOUT_ENTRIES - 1] as usize;
    let oid_table = HEADER_LEN + FANOUT_LEN;
    let entries_len = n
        .checked_mul(ENTRY_LEN)
        .ok_or_else(|| corrupt("object count overflows usize"))?;
    let need = oid_table + entries_len + TRAILER_LEN;
    if data.len() < need {
        return Err(corrupt(format!(
            "fanout declares {n} objects (>= {need} bytes) but the file is only {} bytes",
            data.len()
        )));
    }
    let crc_table = oid_table + n * RAW_LEN;
    let offset_table = crc_table + n * 4;
    let big_table = offset_table + n * 4;
    let trailer = data.len() - TRAILER_LEN;
    let big_bytes = trailer - big_table;
    if big_bytes % 8 != 0 {
        return Err(corrupt(format!(
            "64-bit offset table is {big_bytes} bytes, not a multiple of 8"
        )));
    }
    let big_count = big_bytes / 8;

    let mut oids = Vec::with_capacity(n);
    for i in 0..n {
        let start = oid_table + i * RAW_LEN;
        let mut raw = [0u8; RAW_LEN];
        raw.copy_from_slice(&data[start..start + RAW_LEN]);
        let oid = Oid::from_bytes(raw);
        if let Some(previous) = oids.last() {
            if oid <= *previous {
                return Err(corrupt(format!(
                    "oid table is not strictly ascending (entry {i} = {oid}, previous = {previous})"
                )));
            }
        }
        oids.push(oid);
    }

    // fanout 必须与 oid 表自洽：fanout[i] = 「首字节 <= i 的对象个数」。
    let mut buckets = [0u32; FANOUT_ENTRIES];
    for oid in &oids {
        buckets[usize::from(oid.as_bytes()[0])] += 1;
    }
    let mut running = 0u32;
    for i in 0..FANOUT_ENTRIES {
        running += buckets[i];
        if running != fanout[i] {
            return Err(corrupt(format!(
                "fanout[{i}] = {} disagrees with the oid table (which implies {running})",
                fanout[i]
            )));
        }
    }

    let mut offsets = Vec::with_capacity(n);
    for i in 0..n {
        let raw = u32be(&data[offset_table + i * 4..offset_table + i * 4 + 4]);
        let offset = if raw & BIG_OFFSET_FLAG != 0 {
            let index = (raw & !BIG_OFFSET_FLAG) as usize;
            if index >= big_count {
                return Err(corrupt(format!(
                    "entry {i} points at 64-bit offset #{index}, but the table has only {big_count} entries"
                )));
            }
            let value = u64be(&data[big_table + index * 8..big_table + index * 8 + 8]);
            if value < BIG_OFFSET_MIN {
                return Err(corrupt(format!(
                    "64-bit offset #{index} is {value}, but such offsets must be >= 2^31"
                )));
            }
            value
        } else {
            u64::from(raw)
        };
        offsets.push(offset);
    }

    Ok((oids, offsets))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------ 构造工具

    /// 手工写一个 `.idx` v2（唯一允许硬编码的「格式常量」就是这些东西）。
    fn build(entries: &[(Oid, u64)]) -> Vec<u8> {
        let mut sorted = entries.to_vec();
        sorted.sort_by_key(|(oid, _)| *oid);

        let mut out = Vec::new();
        out.extend_from_slice(IDX_MAGIC);
        out.extend_from_slice(&IDX_VERSION.to_be_bytes());

        let mut buckets = [0u32; FANOUT_ENTRIES];
        for (oid, _) in &sorted {
            buckets[usize::from(oid.as_bytes()[0])] += 1;
        }
        let mut running = 0u32;
        for count in buckets {
            running += count;
            out.extend_from_slice(&running.to_be_bytes());
        }

        for (oid, _) in &sorted {
            out.extend_from_slice(oid.as_bytes());
        }
        for _ in &sorted {
            out.extend_from_slice(&0u32.to_be_bytes()); // crc32：本模块不校验
        }

        let mut big = Vec::new();
        for (_, offset) in &sorted {
            if *offset >= BIG_OFFSET_MIN {
                let index = (big.len() / 8) as u32;
                out.extend_from_slice(&(BIG_OFFSET_FLAG | index).to_be_bytes());
                big.extend_from_slice(&offset.to_be_bytes());
            } else {
                out.extend_from_slice(&(*offset as u32).to_be_bytes());
            }
        }
        out.extend_from_slice(&big);
        // 尾部两个 sha1：pack 的（本模块不校验）+ idx 自身的（resign 会写满）。
        out.extend_from_slice(&[0u8; TRAILER_LEN]);
        resign(&mut out);
        out
    }

    /// 重算尾部 sha1（改坏字节后用，以便测到「结构校验」而不是校验和）。
    fn resign(bytes: &mut [u8]) {
        let len = bytes.len();
        let mut hasher = Sha1::new();
        hasher.update(&bytes[..len - RAW_LEN]);
        let sum = hasher.finalize();
        bytes[len - RAW_LEN..].copy_from_slice(&sum);
    }

    fn oid(hex: &str) -> Oid {
        Oid::from_hex(hex).unwrap()
    }

    fn oid_from_byte(first: u8, seed: usize) -> Oid {
        let mut raw = [0u8; RAW_LEN];
        raw[0] = first;
        raw[1] = seed as u8;
        raw[2] = (seed >> 8) as u8;
        raw[19] = 0x5a;
        Oid::from_bytes(raw)
    }

    fn write_tmp(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pack-test.idx");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    fn open_err(bytes: &[u8]) -> Error {
        let (_dir, path) = write_tmp(bytes);
        PackIndex::open(&path).unwrap_err()
    }

    // ------------------------------------------------------------ 正向

    #[test]
    fn opens_a_synthetic_index_and_looks_up_every_entry() {
        let mut entries = Vec::new();
        for seed in 0..300usize {
            let first = (seed % 256) as u8;
            entries.push((oid_from_byte(first, seed), 12 + seed as u64 * 7 + 1_000_000));
        }
        entries.push((oid("ff00000000000000000000000000000000000000"), 1_234_567));
        let bytes = build(&entries);

        let (_dir, path) = write_tmp(&bytes);
        let index = PackIndex::open(&path).unwrap();
        assert_eq!(index.len(), entries.len());
        assert!(!index.is_empty());
        assert_eq!(index.path.as_deref(), Some(path.as_path()));

        for (oid, offset) in &entries {
            assert_eq!(index.lookup(*oid), Some(*offset), "{oid}");
            assert!(index.iter_oids().contains(oid));
        }
        // 升序且与 offsets 一一对应。
        assert!(index.iter_oids().windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(index.oids.len(), index.offsets.len());

        // 不在索引里的 oid → None。
        assert_eq!(
            index.lookup(oid("0123456789abcdef0123456789abcdef01234567")),
            None
        );
        assert_eq!(index.pack_path(), Some(path.with_extension("pack")));
    }

    #[test]
    fn reads_offsets_from_the_64bit_table() {
        let entries = vec![
            (oid_from_byte(0x10, 1), 12),
            (oid_from_byte(0x20, 2), 0x8000_0000 + 5),
            (oid_from_byte(0x30, 3), 0x1_0000_0000 + 7),
        ];
        let bytes = build(&entries);
        let index = PackIndex::open(&write_tmp(&bytes).1).unwrap();
        assert_eq!(index.lookup(oid_from_byte(0x20, 2)), Some(0x8000_0000 + 5));
        assert_eq!(
            index.lookup(oid_from_byte(0x30, 3)),
            Some(0x1_0000_0000 + 7)
        );
    }

    #[test]
    fn an_empty_index_is_valid_and_empty() {
        let bytes = build(&[]);
        let index = PackIndex::open(&write_tmp(&bytes).1).unwrap();
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
        assert_eq!(index.lookup(oid_from_byte(1, 1)), None);
    }

    // ------------------------------------------------------------ 反向

    #[test]
    fn rejects_bad_magic_and_version() {
        let mut bytes = build(&[(oid_from_byte(1, 1), 12)]);
        bytes[0] = b'P';
        assert!(matches!(open_err(&bytes), Error::Corrupt { .. }));

        let mut bytes = build(&[(oid_from_byte(1, 1), 12)]);
        bytes[4..8].copy_from_slice(&3u32.to_be_bytes());
        resign(&mut bytes);
        assert!(matches!(open_err(&bytes), Error::Corrupt { .. }));
    }

    #[test]
    fn rejects_a_truncated_index() {
        let full = build(&[(oid_from_byte(1, 1), 12), (oid_from_byte(2, 2), 40)]);
        for cut in [1, 20, 40, 8, full.len() - 4] {
            let bytes = &full[..full.len() - cut];
            assert!(
                matches!(open_err(bytes), Error::Corrupt { .. }),
                "truncated by {cut} must be Corrupt"
            );
        }
    }

    #[test]
    fn rejects_a_non_monotonic_fanout() {
        let mut bytes = build(&[(oid_from_byte(0x10, 1), 12), (oid_from_byte(0x20, 2), 40)]);
        // fanout[0] 改成比 fanout[1] 大。
        bytes[HEADER_LEN..HEADER_LEN + 4].copy_from_slice(&9u32.to_be_bytes());
        bytes[HEADER_LEN + 4..HEADER_LEN + 8].copy_from_slice(&1u32.to_be_bytes());
        resign(&mut bytes);
        assert!(matches!(open_err(&bytes), Error::Corrupt { .. }));
    }

    #[test]
    fn rejects_a_fanout_that_disagrees_with_the_oid_table() {
        let mut bytes = build(&[(oid_from_byte(0x10, 1), 12), (oid_from_byte(0x20, 2), 40)]);
        // 把第一个 oid 的首字节从 0x10 改成 0x11：长度与单调性都还成立，只有 fanout 对不上。
        let first_oid = HEADER_LEN + FANOUT_LEN;
        bytes[first_oid] = 0x11;
        resign(&mut bytes);
        assert!(matches!(open_err(&bytes), Error::Corrupt { .. }));
    }

    #[test]
    fn rejects_an_unsorted_oid_table() {
        let mut bytes = build(&[(oid_from_byte(0x10, 1), 12), (oid_from_byte(0x11, 2), 40)]);
        let first_oid = HEADER_LEN + FANOUT_LEN;
        let mut swapped = [0u8; RAW_LEN];
        swapped.copy_from_slice(&bytes[first_oid..first_oid + RAW_LEN]);
        let second = first_oid + RAW_LEN;
        let mut other = [0u8; RAW_LEN];
        other.copy_from_slice(&bytes[second..second + RAW_LEN]);
        bytes[first_oid..first_oid + RAW_LEN].copy_from_slice(&other);
        bytes[second..second + RAW_LEN].copy_from_slice(&swapped);
        resign(&mut bytes);
        assert!(matches!(open_err(&bytes), Error::Corrupt { .. }));
    }

    #[test]
    fn rejects_a_tampered_index_checksum() {
        let mut bytes = build(&[(oid_from_byte(0x10, 1), 12), (oid_from_byte(0x20, 2), 40)]);
        // 只翻改 oid 的中间字节：升序/fanout 都还成立，只有尾部 sha1 抓得到。
        let first_oid = HEADER_LEN + FANOUT_LEN;
        bytes[first_oid + 5] ^= 0x01;
        assert!(matches!(open_err(&bytes), Error::Corrupt { .. }));
    }

    #[test]
    fn rejects_a_big_offset_index_that_is_out_of_range() {
        let entries = [
            (oid_from_byte(0x10, 1), 0x1_0000_0000u64),
            (oid_from_byte(0x20, 2), 0x1_0000_0008u64),
        ];
        let mut bytes = build(&entries);
        // 删掉大 offset 表的第二项，再让第一条 offset 记录指向 #1：索引越界。
        let trailer = bytes.len() - TRAILER_LEN;
        bytes.drain(trailer - 8..trailer);
        let offset_table = HEADER_LEN + FANOUT_LEN + entries.len() * RAW_LEN + entries.len() * 4;
        bytes[offset_table..offset_table + 4].copy_from_slice(&(BIG_OFFSET_FLAG | 1).to_be_bytes());
        resign(&mut bytes);
        assert!(matches!(open_err(&bytes), Error::Corrupt { .. }));
    }

    #[test]
    fn rejects_a_64bit_offset_that_fits_in_32_bits() {
        let mut bytes = build(&[(oid_from_byte(0x10, 1), 0x1_0000_0000)]);
        // 把大 offset 表里的值改成 12：git 不会这么写，读的时候必须报 Corrupt。
        let big = bytes.len() - TRAILER_LEN - 8;
        bytes[big..big + 8].copy_from_slice(&12u64.to_be_bytes());
        resign(&mut bytes);
        assert!(matches!(open_err(&bytes), Error::Corrupt { .. }));
    }
}
