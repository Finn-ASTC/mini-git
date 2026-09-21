//! `mg gc` —— **T15（omp）**。
//!
//! v1 范围（不做 `git gc` 的全部行为：reflog 过期 / prune 策略 / cruft pack /
//! multi-pack-index / delta 压缩策略都不做）：
//! 1. 把**所有 loose 对象**打成一个 `.pack` + `.idx`，放在 `objects/pack/`，
//!    文件名按真实 git 的规则：`pack-<pack 内容的 sha1>.pack` / `.idx`
//!    （实测确认：git 用的就是 pack 尾部那 20 字节校验和，例如
//!    `sha1(pack 去掉尾部 20 字节) == pack-<name>`）；
//! 2. 写/更新 `packed-refs`（`# pack-refs with: peeled fully-peeled sorted ` +
//!    按 refname 排序的 `<oid> <refname>`，annotated tag 后面跟 `^<peeled>` 行 —— 与
//!    `git pack-refs --all` 的字节格式一致）；
//! 3. 删掉**已被打包**的 loose 对象（连同空的 2 位目录）；
//! 4. 幂等：第二次跑时没有 loose 对象，于是既不新建 pack 也不改 packed-refs 的内容。
//!
//! **pack 里不做 delta**（全量存储，与任务书 §5 一致）；真实 git 能正常读取，
//! `git verify-pack -v` / `git fsck` / `git repack -adf` 都通过（见 e2e 与验收记录）。
//! CRC32 按 git 的口径计算：对象记录（varint 头 + zlib 流）的字节。
//!
//! 与真实 git 的已知差异（有意为之，均不影响 git 读我们的 pack）：
//! * 不写 `.rev`（反向索引）/`.bitmap`/`.mtimes`；
//! * 不删 loose ref（只写 `packed-refs`；loose ref 优先，语义完全相同）；
//! * 不同名 pack 的合并：已存在的 pack 一律不动（`git gc` 会 `repack -d` 掉旧的）。

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Crc;
use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::object::{Kind, Tag};
use crate::odb::{loose, Odb};
use crate::oid::Oid;
use crate::refs::{RefStore, TAGS_PREFIX};
use crate::repo::Repo;
use crate::zlib;

/// pack 对象记录的类型码（与 `odb/pack/read.rs` 的读取侧一一对应）。
const TYPE_COMMIT: u8 = 1;
const TYPE_TREE: u8 = 2;
const TYPE_BLOB: u8 = 3;
const TYPE_TAG: u8 = 4;

/// `.idx` v2 里 offset 表用 31 位；`offset >= 2^31` 时必须写成
/// `0x80000000 | 大 offset 表下标`，真值放到大 offset 表（u64be）。
const BIG_OFFSET_FLAG: u32 = 0x8000_0000;
const BIG_OFFSET_MIN: u64 = 0x8000_0000;

/// 临时文件的重试次数（名字里已带 pid/nanos/序号）。
const TEMP_ATTEMPTS: usize = 8;

/// 跟随 tag → tag → … 的最大深度（防环），超过就**不写** peel 行。
const MAX_TAG_DEPTH: usize = 16;

/// 真实 git 对 pack/idx 用的是只读权限（`0444`）。
const FILE_MODE: u32 = 0o444;

pub fn run() -> Result<()> {
    let repo = crate::cli::open_repo()?;
    let odb = Odb::new(&repo);

    let loose_oids = odb.iter_loose()?;
    if loose_oids.is_empty() {
        let refs = write_packed_refs(&repo)?;
        println!(
            "Nothing to pack: no loose objects{}",
            if refs == 0 {
                String::new()
            } else {
                format!(" ({refs} ref(s) written to packed-refs)")
            }
        );
        return Ok(());
    }

    let pack = build_pack(&odb, &loose_oids)?;
    let name = pack.name();
    write_pack(&repo, &name, &pack)?;
    // 只有 pack 与 idx 都落盘之后才删 loose 对象：中途失败也不会丢数据。
    for oid in &loose_oids {
        remove_loose(&repo, *oid)?;
    }
    let refs = write_packed_refs(&repo)?;

    println!(
        "Packed {} loose object(s) into objects/pack/{name}.pack",
        loose_oids.len()
    );
    if refs > 0 {
        println!("Wrote {refs} ref(s) to packed-refs");
    }
    Ok(())
}

// ---------------------------------------------------------------- pack 构造

/// pack 里一个对象的簿记信息（`.idx` 需要 oid / offset / crc32）。
struct PackObject {
    oid: Oid,
    offset: u64,
    crc: u32,
}

struct Pack {
    bytes: Vec<u8>,
    /// pack 尾部校验和（也就是文件名里的那个哈希）。
    checksum: Oid,
    objects: Vec<PackObject>,
}

impl Pack {
    fn name(&self) -> String {
        format!("pack-{}", self.checksum.to_hex())
    }
}

fn build_pack(odb: &Odb, oids: &[Oid]) -> Result<Pack> {
    let count = u32::try_from(oids.len())
        .map_err(|_| Error::Other(format!("too many objects to pack: {}", oids.len())))?;

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"PACK");
    bytes.extend_from_slice(&2u32.to_be_bytes());
    bytes.extend_from_slice(&count.to_be_bytes());

    let mut objects = Vec::with_capacity(oids.len());
    for oid in oids {
        let (kind, payload) = odb.read(*oid)?;
        // 与 fsck 同一判定：内容不自洽的 loose 对象绝不能被打进 pack（那会制造一个坏 pack）。
        let actual = Oid::hash_object(kind.as_str(), &payload);
        if actual != *oid {
            return Err(Error::corrupt(
                format!("object {oid}"),
                format!("its contents hash to {actual}; refusing to pack a corrupt object"),
            ));
        }
        let header = object_header(type_code(kind), payload.len() as u64);
        let compressed = zlib::deflate(&payload)?;

        let mut crc = Crc::new();
        crc.update(&header);
        crc.update(&compressed);

        let offset = bytes.len() as u64;
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&compressed);
        objects.push(PackObject {
            oid: *oid,
            offset,
            crc: crc.sum(),
        });
    }

    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    let checksum = Oid::from_slice(&hasher.finalize())?;
    bytes.extend_from_slice(checksum.as_bytes());

    Ok(Pack {
        bytes,
        checksum,
        objects,
    })
}

/// 对象头：首字节 `(type << 4) | (size & 0x0f)`，后续每字节 7 位（低位在前）。
fn object_header(kind: u8, size: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    let mut byte = (kind << 4) | ((size & 0x0f) as u8);
    let mut rest = size >> 4;
    while rest > 0 {
        out.push(byte | 0x80);
        byte = (rest & 0x7f) as u8;
        rest >>= 7;
    }
    out.push(byte);
    out
}

fn type_code(kind: Kind) -> u8 {
    match kind {
        Kind::Commit => TYPE_COMMIT,
        Kind::Tree => TYPE_TREE,
        Kind::Blob => TYPE_BLOB,
        Kind::Tag => TYPE_TAG,
    }
}

/// `.idx` v2：magic + version + fanout + oid 表 + crc32 表 + offset 表 + 大 offset 表 +
/// （pack 校验和, idx 校验和）。
fn build_index(pack: &Pack) -> Vec<u8> {
    let mut sorted: Vec<&PackObject> = pack.objects.iter().collect();
    sorted.sort_by_key(|object| object.oid);

    let mut out = Vec::new();
    out.extend_from_slice(b"\xfftOc");
    out.extend_from_slice(&2u32.to_be_bytes());

    let mut buckets = [0u32; 256];
    for object in &sorted {
        buckets[usize::from(object.oid.as_bytes()[0])] += 1;
    }
    let mut running = 0u32;
    for bucket in buckets {
        running += bucket;
        out.extend_from_slice(&running.to_be_bytes());
    }
    for object in &sorted {
        out.extend_from_slice(object.oid.as_bytes());
    }
    for object in &sorted {
        out.extend_from_slice(&object.crc.to_be_bytes());
    }

    let mut big = Vec::new();
    for object in &sorted {
        if object.offset >= BIG_OFFSET_MIN {
            let index = (big.len() / 8) as u32;
            out.extend_from_slice(&(BIG_OFFSET_FLAG | index).to_be_bytes());
            big.extend_from_slice(&object.offset.to_be_bytes());
        } else {
            out.extend_from_slice(&(object.offset as u32).to_be_bytes());
        }
    }
    out.extend_from_slice(&big);

    out.extend_from_slice(pack.checksum.as_bytes());
    let mut hasher = Sha1::new();
    hasher.update(&out);
    out.extend_from_slice(&hasher.finalize());
    out
}

// ---------------------------------------------------------------- 落盘

fn write_pack(repo: &Repo, name: &str, pack: &Pack) -> Result<()> {
    let dir = repo.objects_dir().join("pack");
    fs::create_dir_all(&dir)?;
    // 先 pack 后 idx：idx 是「pack 可读了」的标记，缺 idx 时 PackSet 不会把半个 pack 当数据源。
    write_if_missing(&dir, &format!("{name}.pack"), &pack.bytes)?;
    write_if_missing(&dir, &format!("{name}.idx"), &build_index(pack))?;
    Ok(())
}

/// 原子写入（临时文件 + `rename`），只读权限。名字是内容寻址的，已存在即内容相同。
fn write_if_missing(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let path = dir.join(name);
    if path.is_file() {
        return Ok(());
    }
    let mut last: Option<std::io::Error> = None;
    for _ in 0..TEMP_ATTEMPTS {
        let temp = temp_path(dir);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                last = Some(err);
                continue;
            }
            Err(err) => return Err(Error::Io(err)),
        };
        let written = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(Error::Io);
        drop(file);
        if let Err(err) = written {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
        if let Err(err) = set_read_only(&temp) {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
        if let Err(err) = fs::rename(&temp, &path) {
            let _ = fs::remove_file(&temp);
            return Err(Error::Io(err));
        }
        return Ok(());
    }
    Err(Error::Other(format!(
        "could not create a temporary file in {}: {}",
        dir.display(),
        last.map_or_else(|| "unknown error".to_string(), |err| err.to_string())
    )))
}

fn temp_path(dir: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    dir.join(format!("tmp_pack_{}_{nanos}", std::process::id()))
}

fn set_read_only(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// 删掉已经进 pack 的 loose 对象，并顺手清掉空的 2 位目录（真实 git 也这么做）。
fn remove_loose(repo: &Repo, oid: Oid) -> Result<()> {
    let path = loose::loose_path(repo, oid);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(Error::Io(err)),
    }
    if let Some(shard) = path.parent() {
        let _ = fs::remove_dir(shard);
    }
    Ok(())
}

// ---------------------------------------------------------------- packed-refs

/// 写 `packed-refs`（`RefStore::list()` = loose ∪ packed、按名字排序）。
/// 返回写入的 ref 数量；没有 ref 时不动这个文件。
fn write_packed_refs(repo: &Repo) -> Result<usize> {
    let store = RefStore::new(repo);
    let refs = store.list()?;
    if refs.is_empty() {
        return Ok(0);
    }

    let odb = Odb::new(repo);
    // 头部逐字节与 `git pack-refs --all` 相同（注意 "sorted" 后面那个空格）。
    let mut body = String::from("# pack-refs with: peeled fully-peeled sorted \n");
    for (name, oid) in &refs {
        body.push_str(&format!("{oid} {name}\n"));
        if let Some(peeled) = peel_tag(&odb, name, *oid) {
            body.push_str(&format!("^{peeled}\n"));
        }
    }

    let path = repo.git_dir().join("packed-refs");
    let dir = repo.git_dir().to_path_buf();
    write_replace(&dir, &path, body.as_bytes())?;
    Ok(refs.len())
}

/// annotated tag 的 peeled 目标（tag → tag → … 的第一个非 tag 对象）。
/// 只有确实解析到终点才返回 `Some`：头部声明了 `fully-peeled`，写错还不如不写。
fn peel_tag(odb: &Odb, name: &str, oid: Oid) -> Option<Oid> {
    if !name.starts_with(TAGS_PREFIX) {
        return None;
    }
    let mut current = oid;
    let mut peeled = false;
    let mut resolved = false;
    for _ in 0..MAX_TAG_DEPTH {
        match odb.read(current) {
            Ok((Kind::Tag, payload)) => {
                let tag = Tag::decode_payload(&payload).ok()?;
                current = tag.object;
                peeled = true;
            }
            // 非 tag：peel 的终点。
            Ok(_) => {
                resolved = true;
                break;
            }
            // tag 链断了（对象缺失/损坏）：不猜，交给 `mg fsck` 报错。
            Err(_) => return None,
        }
    }
    if peeled && resolved {
        Some(current)
    } else {
        None
    }
}

/// 原子替换（临时文件 + `rename`）；用于 `packed-refs` 这种需要覆盖写的文件。
fn write_replace(dir: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = temp_path(dir);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(Error::Io)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(Error::Io);
    drop(file);
    if let Err(err) = written {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(Error::Io(err));
    }
    Ok(())
}
