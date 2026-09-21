//! DIRC v2 二进制编解码。**T3（opencode）实现范围**。
//!
//! ```text
//! "DIRC" + version:u32be + entry_count:u32be
//! entry[entry_count]
//! extension*            # 4 字节签名 + u32be 长度 + 数据；未知的必须原样保留
//! trailer: sha1(前面所有字节)
//! ```
//!
//! entry 布局（全部大端）：
//!
//! | 字段 | 宽度 |
//! |---|---|
//! | ctime sec / nsec | 4 + 4 |
//! | mtime sec / nsec | 4 + 4 |
//! | dev / ino | 4 + 4 |
//! | mode | 4（含类型位，如 `0100644`）|
//! | uid / gid | 4 + 4 |
//! | size | 4（低 32 位）|
//! | oid | 20（二进制）|
//! | flags | 2（bit15 assume-valid，bit14 extended，bit13-12 stage，低 12 位 name 长度，上限 0xFFF）|
//! | path | 变长，无 NUL |
//! | padding | 1–8 个 NUL，使整个 entry 长度为 8 的倍数 |
//!
//! 验收：`mg` 重写真实 git 的 index 后，`git ls-files --stage` 输出逐字节不变；
//! 并且 `git status` 仍然认为工作区干净。
//!
//! ## 实现要点
//!
//! **无损往返** 是硬指标。entry 的 stat/flags 原样搬运；flags 低 12 位在写回时按
//! 实际 path 长度重算（≥0xFFF 截断为 0xFFF），其余位从 `flags_raw` 继承。
//!
//! 长路径（≥0xFFF）不额外写 NUL：格式要求 entry 末尾至少有 1 个 padding NUL，
//! 读取时从 path 起点找第一个 NUL 即可确定结尾（与真实 git 2.x 行为一致）。
//!
//! `TREE` 扩展在真实 git 里是 **cache-tree**（`name\0` 加上 `entry_count`、`subtree_nr`
//! 两个十进制计数行，再跟 20 字节 oid，递归），通常远大于 20 字节。为了字节级往返，
//! 原始扩展数据必须留在 `Index::extensions` 里原样写回；同时把根 tree oid 解析进
//! `Index::tree_oid` 供上层使用。当 `tree_oid` 存在但 extensions 里没有对应原始数据
//! （例如程序化构造的 index）时，退化为写一个 20 字节 oid 的 TREE 扩展
//! （真实 git 能安全读取）。
//!
//! trailer 是「header 加 entries 加 extensions」的裸 SHA-1（不是 `sha1("blob\0"+…)`）。

use std::fs;

use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::index::{Extension, Index, IndexEntry, StatData, TREE_EXTENSION};
use crate::object::FileMode;
use crate::oid::{Oid, RAW_LEN};
use crate::repo::Repo;

/// DIRC v2 支持的版本号；读到 v3/v4 要明确报 Unsupported，而不是猜着解析。
pub const SUPPORTED_VERSION: u32 = 2;

/// `"DIRC" | version | entry_count`。
const HEADER_LEN: usize = 12;
/// entry 固定头：10 个 u32 + 20 字节 oid + u16 flags。
const ENTRY_FIXED_LEN: usize = 62;
/// 结尾 SHA-1。
const TRAILER_LEN: usize = 20;

/// flags 低 12 位能表示的最大 name 长度。
const NAME_LEN_MAX: usize = 0x0FFF;

const FLAG_ASSUME_VALID: u16 = 0x8000;
const FLAG_EXTENDED: u16 = 0x4000;
const FLAG_STAGE_SHIFT: u16 = 12;
const FLAG_NAME_MASK: u16 = 0x0FFF;

/// 读 `.git/index`；文件不存在时返回空 `Index`（不是错误）。
pub fn read_index(repo: &Repo) -> Result<Index> {
    let path = repo.index_path();
    let data = match fs::read(&path) {
        Ok(data) => data,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Index::default()),
        Err(err) => return Err(Error::Io(err)),
    };
    decode(&data)
}

/// 原子写 `.git/index`：先写 `index.lock`，再 `rename` 覆盖目标。
pub fn write_index(repo: &Repo, index: &Index) -> Result<()> {
    let encoded = encode(index)?;
    let target = repo.index_path();
    let lock = repo.git_dir().join("index.lock");
    fs::write(&lock, &encoded)?;
    match fs::rename(&lock, &target) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&lock);
            Err(Error::Io(err))
        }
    }
}

// ---------------------------------------------------------------------------
// 解码
// ---------------------------------------------------------------------------

fn decode(data: &[u8]) -> Result<Index> {
    if data.len() < HEADER_LEN + TRAILER_LEN {
        return Err(Error::corrupt(
            "index",
            format!("file too short ({} bytes)", data.len()),
        ));
    }
    if &data[0..4] != b"DIRC" {
        return Err(Error::corrupt(
            "index",
            format!("bad signature {:?}", String::from_utf8_lossy(&data[0..4])),
        ));
    }

    let version = be_u32(data, 4)?;
    if version != SUPPORTED_VERSION {
        // `Error::Unsupported` 只收 `&'static str`；版本号需要动态格式化。
        let message: &'static str = Box::leak(
            format!("index version {version} is not supported (v1 supports 2)").into_boxed_str(),
        );
        return Err(Error::Unsupported(message));
    }

    let trailer_start = data.len() - TRAILER_LEN;
    verify_trailer(&data[..trailer_start], &data[trailer_start..])?;

    let entry_count = be_u32(data, 8)? as usize;
    let mut index = Index {
        version,
        entries: Vec::with_capacity(entry_count.min(1 << 16)),
        tree_oid: None,
        extensions: Vec::new(),
    };

    let mut offset = HEADER_LEN;
    for ordinal in 0..entry_count {
        let (entry, next) = decode_entry(data, offset, trailer_start, ordinal)?;
        index.entries.push(entry);
        offset = next;
    }

    while offset < trailer_start {
        if offset + 8 > trailer_start {
            return Err(Error::corrupt("index", "truncated extension header"));
        }
        let mut signature = [0u8; 4];
        signature.copy_from_slice(&data[offset..offset + 4]);
        let length = be_u32(data, offset + 4)? as usize;
        let payload_start = offset + 8;
        let payload_end = payload_start
            .checked_add(length)
            .filter(|end| *end <= trailer_start)
            .ok_or_else(|| Error::corrupt("index", "extension length out of bounds"))?;
        let payload = data[payload_start..payload_end].to_vec();

        if signature == TREE_EXTENSION && index.tree_oid.is_none() {
            index.tree_oid = parse_tree_oid(&payload);
        }
        index.extensions.push(Extension {
            signature,
            data: payload,
        });
        offset = payload_end;
    }

    if offset != trailer_start {
        return Err(Error::corrupt("index", "extension region length mismatch"));
    }

    Ok(index)
}

fn decode_entry(
    data: &[u8],
    start: usize,
    limit: usize,
    ordinal: usize,
) -> Result<(IndexEntry, usize)> {
    if start + ENTRY_FIXED_LEN > limit {
        return Err(Error::corrupt(
            "index",
            format!("entry {ordinal} header runs past end of entries"),
        ));
    }

    let stat = StatData {
        ctime_s: be_u32(data, start)?,
        ctime_ns: be_u32(data, start + 4)?,
        mtime_s: be_u32(data, start + 8)?,
        mtime_ns: be_u32(data, start + 12)?,
        dev: be_u32(data, start + 16)?,
        ino: be_u32(data, start + 20)?,
        uid: be_u32(data, start + 28)?,
        gid: be_u32(data, start + 32)?,
        size: be_u32(data, start + 36)?,
    };
    let mode = FileMode::from_u32(be_u32(data, start + 24)?)?;
    let oid = Oid::from_slice(&data[start + 40..start + 60])?;
    let flags = be_u16(data, start + 60)?;

    let path_start = start + ENTRY_FIXED_LEN;
    let name_field = (flags & FLAG_NAME_MASK) as usize;
    let path_end = if name_field < NAME_LEN_MAX {
        let end = path_start + name_field;
        if end > limit {
            return Err(Error::corrupt(
                "index",
                format!("entry {ordinal} name runs past end of entries"),
            ));
        }
        end
    } else {
        // 长路径：按 NUL 找结尾（entry 末尾的 padding 必然提供这个 NUL）。
        let nul = data[path_start..limit]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| {
                Error::corrupt(
                    "index",
                    format!("entry {ordinal} long name has no NUL terminator"),
                )
            })?;
        path_start + nul
    };

    let fixed_len = path_end - start;
    let entry_end = path_end + (8 - (fixed_len % 8));
    if entry_end > limit {
        return Err(Error::corrupt(
            "index",
            format!("entry {ordinal} padding runs past end of entries"),
        ));
    }

    let entry = IndexEntry {
        path: data[path_start..path_end].to_vec(),
        oid,
        mode,
        stat,
        stage: ((flags >> FLAG_STAGE_SHIFT) & 0x3) as u8,
        assume_valid: flags & FLAG_ASSUME_VALID != 0,
        extended: flags & FLAG_EXTENDED != 0,
        flags_raw: flags,
    };
    Ok((entry, entry_end))
}

/// 从 `TREE` 扩展数据里解析根 tree oid。
///
/// 真实 git 的 cache-tree 形如 `name\0<entry_count> <subtree_nr>\n<oid>`（根 name 为空），
/// 但历史上也存在只放 20 字节 oid 的简化写法；两种都接受，解析失败则返回 `None`
/// （原始字节仍会在 `extensions` 里原样保留，不影响往返）。
fn parse_tree_oid(data: &[u8]) -> Option<Oid> {
    if data.len() == RAW_LEN {
        return Oid::from_slice(data).ok();
    }
    let name_end = data.iter().position(|byte| *byte == 0)?;
    let counts_start = name_end + 1;
    let counts_end = data[counts_start..]
        .iter()
        .position(|byte| *byte == b'\n')?
        + counts_start;
    let oid_start = counts_end + 1;
    let oid_end = oid_start.checked_add(RAW_LEN)?;
    if oid_end > data.len() {
        return None;
    }
    Oid::from_slice(&data[oid_start..oid_end]).ok()
}

fn verify_trailer(body: &[u8], trailer: &[u8]) -> Result<()> {
    let mut hasher = Sha1::new();
    hasher.update(body);
    let digest = hasher.finalize();
    if digest.as_slice() != trailer {
        return Err(Error::corrupt("index", "trailer SHA-1 mismatch"));
    }
    Ok(())
}

fn be_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| Error::corrupt("index", format!("unexpected end at offset {offset}")))?;
    let mut raw = [0u8; 4];
    raw.copy_from_slice(bytes);
    Ok(u32::from_be_bytes(raw))
}

fn be_u16(data: &[u8], offset: usize) -> Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| Error::corrupt("index", format!("unexpected end at offset {offset}")))?;
    let mut raw = [0u8; 2];
    raw.copy_from_slice(bytes);
    Ok(u16::from_be_bytes(raw))
}

// ---------------------------------------------------------------------------
// 编码
// ---------------------------------------------------------------------------

fn encode(index: &Index) -> Result<Vec<u8>> {
    let entry_count: u32 = index
        .entries
        .len()
        .try_into()
        .map_err(|_| Error::Other("too many index entries".to_string()))?;

    let mut out = Vec::new();
    out.extend_from_slice(b"DIRC");
    out.extend_from_slice(&SUPPORTED_VERSION.to_be_bytes());
    out.extend_from_slice(&entry_count.to_be_bytes());

    for entry in &index.entries {
        encode_entry(&mut out, entry)?;
    }

    // 扩展区：TREE 在逻辑上应当排在最前。若 extensions 里没有原始 TREE 数据
    // （例如程序化构造的 index 只设置了 tree_oid），就先生成一个。
    let has_raw_tree = index
        .extensions
        .iter()
        .any(|ext| ext.signature == TREE_EXTENSION);
    if !has_raw_tree {
        if let Some(oid) = index.tree_oid {
            write_extension(&mut out, TREE_EXTENSION, oid.as_bytes())?;
        }
    }

    for ext in &index.extensions {
        if ext.signature == TREE_EXTENSION {
            match index.tree_oid {
                // 上层改了 tree_oid：用新值覆盖旧的 cache-tree 缓存。
                Some(oid) if parse_tree_oid(&ext.data) != Some(oid) => {
                    write_extension(&mut out, TREE_EXTENSION, oid.as_bytes())?;
                }
                // 往返：原样写回（含完整 cache-tree 字节）。
                _ => write_extension(&mut out, TREE_EXTENSION, &ext.data)?,
            }
        } else {
            write_extension(&mut out, ext.signature, &ext.data)?;
        }
    }

    let mut hasher = Sha1::new();
    hasher.update(&out);
    out.extend_from_slice(&hasher.finalize());
    Ok(out)
}

fn encode_entry(out: &mut Vec<u8>, entry: &IndexEntry) -> Result<()> {
    let stat = &entry.stat;
    out.extend_from_slice(&stat.ctime_s.to_be_bytes());
    out.extend_from_slice(&stat.ctime_ns.to_be_bytes());
    out.extend_from_slice(&stat.mtime_s.to_be_bytes());
    out.extend_from_slice(&stat.mtime_ns.to_be_bytes());
    out.extend_from_slice(&stat.dev.to_be_bytes());
    out.extend_from_slice(&stat.ino.to_be_bytes());
    out.extend_from_slice(&entry.mode.to_u32().to_be_bytes());
    out.extend_from_slice(&stat.uid.to_be_bytes());
    out.extend_from_slice(&stat.gid.to_be_bytes());
    out.extend_from_slice(&stat.size.to_be_bytes());
    out.extend_from_slice(entry.oid.as_bytes());

    let name_len = entry.path.len();
    let name_field = if name_len >= NAME_LEN_MAX {
        NAME_LEN_MAX as u16
    } else {
        name_len as u16
    };
    let mut flags = entry.flags_raw & !FLAG_NAME_MASK;
    if entry.assume_valid {
        flags |= FLAG_ASSUME_VALID;
    }
    if entry.extended {
        flags |= FLAG_EXTENDED;
    }
    flags |= ((entry.stage as u16) & 0x3) << FLAG_STAGE_SHIFT;
    flags |= name_field;
    out.extend_from_slice(&flags.to_be_bytes());

    out.extend_from_slice(&entry.path);
    let size = ENTRY_FIXED_LEN + name_len;
    let padded = size + (8 - (size % 8));
    out.resize(out.len() + (padded - size), 0);
    Ok(())
}

fn write_extension(out: &mut Vec<u8>, signature: [u8; 4], data: &[u8]) -> Result<()> {
    let length: u32 = data
        .len()
        .try_into()
        .map_err(|_| Error::Other("index extension too large".to_string()))?;
    out.extend_from_slice(&signature);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(data);
    Ok(())
}

// ---------------------------------------------------------------------------
// 测试：真值全部来自运行时启动的真实 `git`，不手写 golden 字节。
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    fn git(dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0800")
            .env("GIT_COMMITTER_DATE", "1700000000 +0800")
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .args(args)
            .output()
            .expect("failed to spawn git")
    }

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = git(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    fn init_repo(dir: &Path) {
        git_ok(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
        git_ok(dir, &["config", "user.name", "Test"]);
        git_ok(dir, &["config", "user.email", "test@example.com"]);
    }

    fn discover(dir: &Path) -> Repo {
        Repo::discover(dir).expect("Repo::discover")
    }

    fn index_bytes(dir: &Path) -> Vec<u8> {
        fs::read(dir.join(".git/index")).expect("read .git/index")
    }

    /// 构造一个长度精确为 `total`（`/` 分隔）的相对路径，返回 `(目录部分, 完整路径)`。
    fn long_relative_path(total: usize) -> (String, String) {
        let components = 16usize;
        let width = 240usize;
        let mut parts = Vec::with_capacity(components);
        for i in 0..components {
            let mut name = format!("d{i:03}");
            name.push_str(&"a".repeat(width - name.len()));
            parts.push(name);
        }
        let dir = parts.join("/");
        let file_len = total - 1 - dir.len();
        let mut file = String::from("f");
        file.push_str(&"b".repeat(file_len - 1));
        let full = format!("{dir}/{file}");
        assert_eq!(
            full.len(),
            total,
            "relative path must be exactly {total} bytes"
        );
        (dir, full)
    }

    /// 在仓库根下用相对路径创建超长文件（绝对路径会撞 PATH_MAX，故交给 shell 以 cwd 相对创建）。
    fn write_long_path(dir: &Path, total: usize) {
        let (parent, full) = long_relative_path(total);
        let script = format!("mkdir -p -- '{parent}' && printf deep > '{full}'");
        let out = Command::new("bash")
            .current_dir(dir)
            .arg("-c")
            .arg(&script)
            .output()
            .expect("failed to spawn bash");
        assert!(
            out.status.success(),
            "creating {total}-byte path failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn build_diverse_worktree(dir: &Path) {
        fs::create_dir_all(dir.join("nested/dir")).unwrap();
        fs::write(dir.join("nested/dir/file.txt"), b"hello\n").unwrap();
        fs::write(dir.join("exec.sh"), b"#!/bin/sh\necho hi\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.join("exec.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("nested/dir/file.txt", dir.join("link")).unwrap();
        fs::write(dir.join("中文文件.txt"), b"utf8\n").unwrap();
        write_long_path(dir, 4095);
    }

    fn scratch(label: &str) -> (tempfile::TempDir, PathBuf) {
        let _ = label;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        (tmp, dir)
    }

    #[test]
    fn diverse_index_roundtrips_byte_for_byte() {
        let (_tmp, dir) = scratch("diverse");
        init_repo(&dir);
        build_diverse_worktree(&dir);
        git_ok(&dir, &["add", "-A"]);

        let before = index_bytes(&dir);
        let repo = discover(&dir);
        let index = read_index(&repo).unwrap();

        assert_eq!(index.version, SUPPORTED_VERSION);
        assert_eq!(index.entries.len(), 5, "nested, exec, symlink, utf8, long");
        assert!(
            index.tree_oid.is_none(),
            "`git add` alone must not write a TREE extension"
        );
        assert_eq!(index.lookup(b"exec.sh").unwrap().mode, FileMode::Executable);
        assert_eq!(index.lookup(b"link").unwrap().mode, FileMode::Symlink);
        let long = index
            .entries
            .iter()
            .find(|entry| entry.path.len() >= NAME_LEN_MAX)
            .expect("a >=0xFFF path entry");
        assert_eq!(long.path.len(), 4095);
        assert_eq!(long.flags_raw & FLAG_NAME_MASK, 0x0FFF);

        let ls_before = git_ok(&dir, &["ls-files", "--stage"]);

        write_index(&repo, &index).unwrap();
        assert_eq!(
            index_bytes(&dir),
            before,
            "read -> write must preserve every single byte"
        );

        assert_eq!(git_ok(&dir, &["ls-files", "--stage"]), ls_before);
        git_ok(&dir, &["update-index", "--refresh"]);
        git_ok(&dir, &["fsck", "--no-progress"]);
    }

    #[test]
    fn committed_index_roundtrips_and_exposes_tree_oid() {
        let (_tmp, dir) = scratch("committed");
        init_repo(&dir);
        build_diverse_worktree(&dir);
        git_ok(&dir, &["add", "-A"]);
        git_ok(&dir, &["commit", "-q", "-m", "init"]);
        let head_tree = git_ok(&dir, &["rev-parse", "HEAD^{tree}"]);

        let before = index_bytes(&dir);
        let repo = discover(&dir);
        let index = read_index(&repo).unwrap();
        assert_eq!(
            index.tree_oid,
            Some(Oid::from_hex(&head_tree).unwrap()),
            "cache-tree root oid must equal HEAD^{{tree}}"
        );

        let ls_before = git_ok(&dir, &["ls-files", "--stage"]);
        assert_eq!(git_ok(&dir, &["status", "--porcelain"]), "");

        write_index(&repo, &index).unwrap();
        assert_eq!(
            index_bytes(&dir),
            before,
            "committed index (with full cache-tree) must roundtrip byte for byte"
        );
        assert_eq!(git_ok(&dir, &["ls-files", "--stage"]), ls_before);
        assert_eq!(git_ok(&dir, &["status", "--porcelain"]), "");

        // real git must keep working on the rewritten index
        git_ok(&dir, &["update-index", "--refresh"]);
        fs::write(dir.join("later.txt"), b"later\n").unwrap();
        git_ok(&dir, &["add", "later.txt"]);
        assert!(git_ok(&dir, &["status", "--porcelain"]).contains("later.txt"));
        git_ok(&dir, &["fsck", "--no-progress"]);
    }

    #[test]
    fn unknown_extension_is_preserved_verbatim() {
        let (_tmp, dir) = scratch("unknown-ext");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), b"a\n").unwrap();
        git_ok(&dir, &["add", "-A"]);

        // 在 trailer 之前插入一个真实 git 不认识的扩展。
        let original = index_bytes(&dir);
        let body = &original[..original.len() - TRAILER_LEN];
        let payload = b"an-extension-git-does-not-know";
        let mut crafted = body.to_vec();
        crafted.extend_from_slice(b"ZZzz");
        crafted.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        crafted.extend_from_slice(payload);
        let mut hasher = Sha1::new();
        hasher.update(&crafted);
        crafted.extend_from_slice(&hasher.finalize());
        fs::write(dir.join(".git/index"), &crafted).unwrap();

        let repo = discover(&dir);
        let index = read_index(&repo).unwrap();
        let ext = index
            .extensions
            .iter()
            .find(|ext| &ext.signature == b"ZZzz")
            .expect("unknown extension must be kept");
        assert_eq!(ext.data.as_slice(), payload);

        write_index(&repo, &index).unwrap();
        assert_eq!(
            index_bytes(&dir),
            crafted,
            "unknown extension must be re-emitted verbatim"
        );
    }

    #[test]
    fn generated_tree_extension_roundtrips() {
        let (_tmp, dir) = scratch("gen-tree");
        let repo = Repo::init(&dir, "main").unwrap();
        let oid = Oid::hash_object("blob", b"y");
        let empty_tree = Oid::hash_object("tree", b"");

        let mut index = Index::default();
        index.upsert(IndexEntry::new(b"a.txt".to_vec(), oid, FileMode::Regular));
        index.tree_oid = Some(empty_tree);
        write_index(&repo, &index).unwrap();

        let read_back = read_index(&repo).unwrap();
        assert_eq!(read_back.tree_oid, Some(empty_tree));
        assert!(read_back
            .extensions
            .iter()
            .any(|ext| &ext.signature == b"TREE"));

        let bytes = fs::read(repo.index_path()).unwrap();
        write_index(&repo, &read_back).unwrap();
        assert_eq!(fs::read(repo.index_path()).unwrap(), bytes);
    }

    #[test]
    fn missing_index_reads_empty_and_writes_git_readable_index() {
        let (_tmp, dir) = scratch("empty");
        init_repo(&dir);
        assert!(!dir.join(".git/index").exists());

        let repo = discover(&dir);
        let empty = read_index(&repo).unwrap();
        assert_eq!(empty.version, 0);
        assert!(empty.entries.is_empty());
        assert!(empty.tree_oid.is_none());
        assert!(empty.extensions.is_empty());

        write_index(&repo, &empty).unwrap();
        assert_eq!(index_bytes(&dir).len(), HEADER_LEN + TRAILER_LEN);
        assert_eq!(git_ok(&dir, &["ls-files", "--stage"]), "");
        assert_eq!(git_ok(&dir, &["status", "--porcelain"]), "");
    }

    #[test]
    fn long_path_name_length_boundaries_roundtrip() {
        let (_tmp, dir) = scratch("long-boundary");
        let repo = Repo::init(&dir, "main").unwrap();
        let oid = Oid::hash_object("blob", b"x");

        let lengths = [0x0FFEu32, 0x0FFF, 0x1000];
        let mut index = Index::default();
        for len in lengths {
            index.upsert(IndexEntry::new(
                vec![b'a'; len as usize],
                oid,
                FileMode::Regular,
            ));
        }
        write_index(&repo, &index).unwrap();

        let bytes = fs::read(repo.index_path()).unwrap();
        let read_back = read_index(&repo).unwrap();
        assert_eq!(read_back.entries.len(), lengths.len());
        for (entry, len) in read_back.entries.iter().zip(lengths) {
            assert_eq!(entry.path.len(), len as usize);
            assert_eq!(entry.oid, oid);
        }

        write_index(&repo, &read_back).unwrap();
        assert_eq!(
            fs::read(repo.index_path()).unwrap(),
            bytes,
            "4094/4095/4096-byte names must roundtrip"
        );
    }

    #[test]
    fn corrupt_trailer_is_rejected() {
        let (_tmp, dir) = scratch("corrupt-trailer");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), b"a\n").unwrap();
        git_ok(&dir, &["add", "-A"]);

        let mut bytes = index_bytes(&dir);
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        fs::write(dir.join(".git/index"), &bytes).unwrap();

        let err = read_index(&discover(&dir)).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "expected Corrupt, got {err:?}"
        );
    }

    #[test]
    fn unsupported_version_is_reported() {
        let (_tmp, dir) = scratch("bad-version");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), b"a\n").unwrap();
        git_ok(&dir, &["add", "-A"]);

        let mut bytes = index_bytes(&dir);
        bytes[4..8].copy_from_slice(&3u32.to_be_bytes());
        fs::write(dir.join(".git/index"), &bytes).unwrap();

        let err = read_index(&discover(&dir)).unwrap_err();
        assert!(
            matches!(err, Error::Unsupported(_)),
            "expected Unsupported, got {err:?}"
        );
    }

    #[test]
    fn bad_signature_and_truncation_error_instead_of_panicking() {
        let (_tmp, dir) = scratch("truncated");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), b"a\n").unwrap();
        git_ok(&dir, &["add", "-A"]);
        let valid = index_bytes(&dir);
        let repo = discover(&dir);

        for len in [0usize, 4, 12, HEADER_LEN + TRAILER_LEN - 1] {
            fs::write(dir.join(".git/index"), &valid[..len]).unwrap();
            let err = read_index(&repo).unwrap_err();
            assert!(
                matches!(err, Error::Corrupt { .. }),
                "len {len}: expected Corrupt, got {err:?}"
            );
        }

        let mut bad_magic = valid.clone();
        bad_magic[0] = b'X';
        fs::write(dir.join(".git/index"), &bad_magic).unwrap();
        let err = read_index(&repo).unwrap_err();
        assert!(matches!(err, Error::Corrupt { .. }), "got {err:?}");
    }

    #[test]
    fn conflicted_index_with_stages_roundtrips() {
        let (_tmp, dir) = scratch("conflict");
        init_repo(&dir);
        fs::write(dir.join("f.txt"), b"base\n").unwrap();
        git_ok(&dir, &["add", "-A"]);
        git_ok(&dir, &["commit", "-q", "-m", "base"]);
        git_ok(&dir, &["checkout", "-q", "-b", "side"]);
        fs::write(dir.join("f.txt"), b"side\n").unwrap();
        git_ok(&dir, &["commit", "-q", "-am", "side"]);
        git_ok(&dir, &["checkout", "-q", "main"]);
        fs::write(dir.join("f.txt"), b"main\n").unwrap();
        git_ok(&dir, &["commit", "-q", "-am", "main"]);

        let merge = git(&dir, &["merge", "side"]);
        assert!(!merge.status.success(), "merge must conflict");

        let repo = discover(&dir);
        let index = read_index(&repo).unwrap();
        assert!(index.has_conflicts(), "conflicted index must expose stages");
        let stages: Vec<u8> = index.entries.iter().map(|entry| entry.stage).collect();
        for stage in [1u8, 2, 3] {
            assert!(stages.contains(&stage), "missing stage {stage}: {stages:?}");
        }

        let before = index_bytes(&dir);
        write_index(&repo, &index).unwrap();
        assert_eq!(
            index_bytes(&dir),
            before,
            "conflicted index must roundtrip byte for byte"
        );
    }

    #[test]
    fn assume_valid_flag_roundtrips() {
        let (_tmp, dir) = scratch("assume-valid");
        init_repo(&dir);
        fs::write(dir.join("a.txt"), b"a\n").unwrap();
        git_ok(&dir, &["add", "-A"]);
        git_ok(&dir, &["update-index", "--assume-unchanged", "a.txt"]);

        let before = index_bytes(&dir);
        let repo = discover(&dir);
        let index = read_index(&repo).unwrap();
        let entry = index.lookup(b"a.txt").unwrap();
        assert!(entry.assume_valid);
        assert_ne!(entry.flags_raw & FLAG_ASSUME_VALID, 0);

        write_index(&repo, &index).unwrap();
        assert_eq!(index_bytes(&dir), before);
    }

    #[test]
    fn tree_and_unknown_extensions_keep_order() {
        let (_tmp, dir) = scratch("mixed-ext");
        init_repo(&dir);
        build_diverse_worktree(&dir);
        git_ok(&dir, &["add", "-A"]);
        git_ok(&dir, &["commit", "-q", "-m", "init"]);

        // Append a second extension after git's TREE cache-tree.
        let original = index_bytes(&dir);
        let payload = b"reuc-like";
        let mut body = original[..original.len() - TRAILER_LEN].to_vec();
        body.extend_from_slice(b"REUC");
        body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        body.extend_from_slice(payload);
        let mut hasher = Sha1::new();
        hasher.update(&body);
        body.extend_from_slice(&hasher.finalize());
        fs::write(dir.join(".git/index"), &body).unwrap();

        let repo = discover(&dir);
        let index = read_index(&repo).unwrap();
        assert!(index.tree_oid.is_some());
        let signatures: Vec<[u8; 4]> = index.extensions.iter().map(|ext| ext.signature).collect();
        assert_eq!(signatures, vec![*b"TREE", *b"REUC"]);

        write_index(&repo, &index).unwrap();
        assert_eq!(index_bytes(&dir), body, "extension order must be preserved");
    }
}
