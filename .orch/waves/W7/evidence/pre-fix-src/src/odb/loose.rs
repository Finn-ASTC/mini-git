//! loose object 读写。**T5（hermes）实现范围**。
//!
//! 路径：`.git/objects/<hex[0:2]>/<hex[2:40]>`。
//! 内容：`deflate_kind = zlib(encode(kind, payload))`。
//! 写入必须原子（临时文件 + rename）且幂等（已存在则直接跳过，不覆盖）。
//!
//! 验收：`mg hash-object -w f` 后 `git cat-file -p <oid>` 正确且 `git fsck` 无报错；
//! 反向，真实 git 写下的 loose object，`mg cat-file -p` 能读出。

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::object::{self, Kind};
use crate::oid::Oid;
use crate::repo::Repo;
use crate::zlib;

/// 真实 git 的 loose 对象权限：文件 `0444`（只读）、目录 `0755`。
/// 只读文件让「同一个对象写第二次」天然成为 no-op，也是 git 的既有行为。
const FILE_MODE: u32 = 0o444;
const DIR_MODE: u32 = 0o755;

/// 临时文件的尝试次数（名字里已经带了 pid/nanos/序号，冲突几乎不可能）。
const TEMP_ATTEMPTS: usize = 8;

pub fn loose_path(repo: &Repo, oid: Oid) -> PathBuf {
    repo.objects_dir().join(oid.loose_rel_path())
}

/// 读取并解压，返回 `(kind, payload)`；不存在时返回 `Ok(None)`。
///
/// 校验链（任一不满足都是 `Error::Corrupt`，与真实 git 的「库损坏」判定一致）：
/// 1. zlib 流必须完整解完（截断文件不是「短对象」而是损坏，由 `zlib::inflate_all` 报错）；
/// 2. header 必须是 `<kind> <len>\0`，且 `len` 等于实际 payload 长度；
/// 3. `sha1("<kind> <len>\0<payload>")` 必须等于文件名里的 oid —— 否则 git 会认为库损坏。
pub fn read_loose(repo: &Repo, oid: Oid) -> Result<Option<(Kind, Vec<u8>)>> {
    let Some(raw) = read_loose_raw(repo, oid)? else {
        return Ok(None);
    };
    let path = loose_path(repo, oid);
    let (kind, payload) = object::decode(&raw).map_err(|err| {
        Error::corrupt(
            path.display().to_string(),
            format!("invalid object header: {err}"),
        )
    })?;
    let actual = Oid::hash_object(kind.as_str(), &payload);
    if actual != oid {
        return Err(Error::corrupt(
            path.display().to_string(),
            format!("object id mismatch: file name is {oid}, contents hash to {actual}"),
        ));
    }
    Ok(Some((kind, payload)))
}

/// 返回**未解压**的 `"<type> <size>\0<payload>"` 字节，供 `mg fsck` 重算 id。
///
/// 截断/损坏的 zlib 流由 `zlib::inflate_all` 直接报错（flate2 对不完整的 deflate 流返回
/// `incomplete deflate stream`），因此这里得到的是 `Corrupt`；**不做** header 与 oid 校验：
/// `Odb::verify` 需要拿到字节自己重算哈希，从而把「内容与文件名不符」判成 `Ok(false)`
/// （而不是错误）；oid 校验由 `read_loose` 承担。
pub fn read_loose_raw(repo: &Repo, oid: Oid) -> Result<Option<Vec<u8>>> {
    let path = loose_path(repo, oid);
    let compressed = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::Io(err)),
    };
    let raw = zlib::inflate_all(&compressed)
        .map_err(|err| Error::corrupt(path.display().to_string(), format!("{err}")))?;
    Ok(Some(raw))
}

/// 写入 loose object，返回其 oid（已存在则直接返回）。
///
/// * 幂等：目标文件已在（内容寻址，内容必然相同）就不再动它，连 mtime/inode 都不变。
/// * 原子：先写 `objects/tmp_obj_<pid>_<nanos>_<seq>`，`fsync` 后 `rename` 到最终路径。
/// * 权限：文件 `0444`、目录 `0755`（与真实 git 相同）。
pub fn write_loose(repo: &Repo, kind: Kind, payload: &[u8]) -> Result<Oid> {
    let oid = Oid::hash_object(kind.as_str(), payload);
    let path = loose_path(repo, oid);
    if path.is_file() {
        return Ok(oid);
    }
    let compressed = zlib::deflate(&object::encode(kind, payload))?;

    let objects = repo.objects_dir();
    let dir = objects.join(&oid.to_hex()[..2]);
    fs::create_dir_all(&dir)?;
    set_mode(&dir, DIR_MODE)?;

    let mut last: Option<std::io::Error> = None;
    for _ in 0..TEMP_ATTEMPTS {
        let tmp = temp_path(&objects);
        match write_temp(&tmp, &compressed) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                last = Some(err);
                continue;
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                return Err(Error::Io(err));
            }
        }
        if let Err(err) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(Error::Io(err));
        }
        return Ok(oid);
    }
    Err(Error::Other(format!(
        "could not create a temporary object file in {}: {}",
        objects.display(),
        last.map_or_else(|| "unknown error".to_string(), |err| err.to_string())
    )))
}

/// 遍历所有 loose object id（跳过 `info/` 与 `pack/`）。结果按 oid 排序、去重。
///
/// 只认「2 位十六进制目录 + 38 位十六进制文件名」的形态；`info/`、`pack/`、
/// 临时文件（`tmp_obj_*`）以及任何非法名字都被跳过。
pub fn iter_loose(repo: &Repo) -> Result<Vec<Oid>> {
    let objects = repo.objects_dir();
    let entries = match fs::read_dir(&objects) {
        Ok(entries) => entries,
        // 空仓库（或尚未创建 objects/）不是错误，只是没有对象。
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::Io(err)),
    };

    let mut shards: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.len() != 2 || !is_hex(name) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        shards.push((name.to_string(), path));
    }
    shards.sort();

    let mut oids = Vec::new();
    for (prefix, shard) in shards {
        for entry in fs::read_dir(&shard)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.len() != 38 || !is_hex(name) {
                continue;
            }
            if !entry.file_type()?.is_file() {
                continue;
            }
            oids.push(Oid::from_hex(&format!("{prefix}{name}"))?);
        }
    }
    oids.sort();
    oids.dedup();
    Ok(oids)
}

/// 2 位 / 38 位名字片段必须是十六进制。
fn is_hex(text: &str) -> bool {
    text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// 把压缩后的对象字节写进临时文件：独占创建（`create_new`）+ 写满 + `fsync` + 变只读。
fn write_temp(tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    match set_mode(tmp, FILE_MODE) {
        Ok(()) => Ok(()),
        Err(err) => Err(std::io::Error::other(err.to_string())),
    }
}

/// `objects/tmp_obj_<pid>_<nanos>_<seq>`：与真实 git 的 `tmp_obj_XXXXXX` 同族，
/// 名字不是合法的 40 位十六进制，所以永远不会被 `iter_loose` 当成对象。
fn temp_path(objects_dir: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.subsec_nanos())
        .unwrap_or(0);
    objects_dir.join(format!("tmp_obj_{}_{}_{}", std::process::id(), nanos, seq))
}

/// 设置权限（git 用 `0444` 文件 / `0755` 目录）。非 unix 平台上退化为 no-op。
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;
    use std::process::{Command, Output};

    use super::*;
    use crate::object::{FileMode, Tree, TreeEntry};
    use crate::odb::Odb;

    // -------------------------------------------------------------- git 测试床

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// 真值只能来自 git 二进制；配置/身份/日期全部就地隔离，绝不 export 进共享 shell。
    fn git_raw(dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "T5")
            .env("GIT_AUTHOR_EMAIL", "t5@example.com")
            .env("GIT_COMMITTER_NAME", "T5")
            .env("GIT_COMMITTER_EMAIL", "t5@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0800")
            .env("GIT_COMMITTER_DATE", "1700000000 +0800")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .args(args)
            .output()
            .expect("failed to spawn git")
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = git_raw(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
        let out = git_raw(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }

    /// 一个有提交的真实 git 仓库（含 blob/tree/commit 三类对象）。
    fn init_git_repo(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("a.txt"), "hello from git\n").unwrap();
        git(dir, &["add", "a.txt"]);
        git(dir, &["commit", "-q", "-m", "initial"]);
    }

    fn oid(hex: &str) -> Oid {
        Oid::from_hex(hex).unwrap()
    }

    fn repo_at(dir: &Path) -> Repo {
        Repo::discover(dir).unwrap()
    }

    /// `objects/` 下是否残留临时文件（写入路径必须自己清理）。
    fn temp_files(repo: &Repo) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in fs::read_dir(repo.objects_dir()).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if name.starts_with("tmp") {
                found.push(path);
            }
        }
        found
    }

    // ------------------------------------------------------------- 1. 读真 git

    /// 验收 1：真实 git `hash-object -w` / `commit` 写下的对象，`read_loose` 必须读出同样的字节。
    #[test]
    fn reads_every_object_kind_real_git_wrote() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_git_repo(dir);
        fs::write(dir.join("payload.bin"), b"hello from real git\n").unwrap();
        let blob_hex = git(dir, &["hash-object", "-w", "payload.bin"]);
        let commit_hex = git(dir, &["rev-parse", "HEAD"]);
        let tree_hex = git(dir, &["rev-parse", "HEAD^{tree}"]);
        let repo = repo_at(dir);

        for hex in [&blob_hex, &commit_hex, &tree_hex] {
            let oid = oid(hex);
            let type_name = git(dir, &["cat-file", "-t", hex]);
            let expected_kind = match type_name.as_str() {
                "blob" => Kind::Blob,
                "tree" => Kind::Tree,
                "commit" => Kind::Commit,
                other => panic!("unexpected kind {other}"),
            };
            // 真值 A：`git cat-file -s` = payload 长度（不含 header）。
            let expected_len: usize = git(dir, &["cat-file", "-s", hex]).parse().unwrap();
            // 真值 B：`git cat-file <type> <oid>` = **未经 pretty** 的原始载荷。
            let expected_payload = git_bytes(dir, &["cat-file", &type_name, hex]);
            assert_eq!(expected_payload.len(), expected_len);

            let (kind, payload) = read_loose(&repo, oid)
                .unwrap()
                .expect("object written by real git must be readable");
            assert_eq!(kind, expected_kind, "{hex}");
            assert_eq!(payload, expected_payload, "{hex} payload differs from git");
            // 非 tree 对象：`git cat-file -p` 的输出就是载荷原文（tree 的 -p 是 ls-tree 形态）。
            if expected_kind != Kind::Tree {
                assert_eq!(git_bytes(dir, &["cat-file", "-p", hex]), payload, "{hex}");
            }
            assert_eq!(
                read_loose_raw(&repo, oid).unwrap().unwrap(),
                object::encode(kind, &payload),
                "{hex}: raw bytes must be `<kind> <len>\\0<payload>`"
            );
        }

        // blob 的 payload 与工作区文件逐字节相同。
        let (_, payload) = read_loose(&repo, oid(&blob_hex)).unwrap().unwrap();
        assert_eq!(payload, fs::read(dir.join("payload.bin")).unwrap());
    }

    // ------------------------------------------------------------ 2. 写 → git

    /// 验收 2：mg 写出的 blob/tree/commit/tag，真实 git 能读，且 `git fsck` 无报错。
    #[test]
    fn objects_mg_wrote_are_readable_by_real_git() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_git_repo(dir);
        let repo = repo_at(dir);

        let blob_payload = b"mg wrote this\n".to_vec();
        let blob = write_loose(&repo, Kind::Blob, &blob_payload).unwrap();

        let tree = Tree::new(vec![TreeEntry {
            mode: FileMode::Regular,
            name: b"x.txt".to_vec(),
            oid: blob,
        }]);
        let tree_payload = tree.encode_payload();
        let tree_oid = write_loose(&repo, Kind::Tree, &tree_payload).unwrap();

        let commit_payload = format!(
            "tree {tree_oid}\nauthor T5 <t5@example.com> 1700000000 +0800\ncommitter T5 <t5@example.com> 1700000000 +0800\n\nfrom mg\n"
        )
        .into_bytes();
        let commit_oid = write_loose(&repo, Kind::Commit, &commit_payload).unwrap();

        // annotated tag：指向上面这个 commit（否则 `git fsck` 会报 missing）。
        let tag_payload = format!(
            "object {commit_oid}\ntype commit\ntag v9\ntagger T5 <t5@example.com> 1700000000 +0800\n\ntag written by mg\n"
        )
        .into_bytes();
        let tag_oid = write_loose(&repo, Kind::Tag, &tag_payload).unwrap();

        let blob_hex = blob.to_hex();
        let tree_hex = tree_oid.to_hex();
        let commit_hex = commit_oid.to_hex();
        let tag_hex = tag_oid.to_hex();
        for (hex, kind, payload) in [
            (blob_hex.as_str(), "blob", blob_payload.as_slice()),
            (tree_hex.as_str(), "tree", tree_payload.as_slice()),
            (commit_hex.as_str(), "commit", commit_payload.as_slice()),
            (tag_hex.as_str(), "tag", tag_payload.as_slice()),
        ] {
            assert_eq!(git(dir, &["cat-file", "-t", hex]), kind, "{hex}");
            assert_eq!(
                git(dir, &["cat-file", "-s", hex]).parse::<usize>().unwrap(),
                payload.len(),
                "{hex}"
            );
            // 原始载荷（不经 pretty）：`git cat-file <type> <oid>`。
            assert_eq!(
                git_bytes(dir, &["cat-file", kind, hex]),
                payload,
                "{hex}: git read different bytes"
            );
            // 非 tree：`-p` 的输出 == 载荷（tree 的 -p 是 ls-tree 形态，留到 cat-file 的测试里验）。
            if kind != "tree" {
                assert_eq!(
                    git_bytes(dir, &["cat-file", "-p", hex]),
                    payload,
                    "{hex}: git pretty output differs"
                );
            }
        }

        // `git fsck` 必须干净：没有 error/corrupt/missing（dangling 提示是正常的）。
        let fsck = git_raw(dir, &["fsck", "--no-progress"]);
        let stderr = String::from_utf8_lossy(&fsck.stderr);
        assert!(fsck.status.success(), "git fsck failed: {stderr}");
        assert!(
            !stderr.contains("error") && !stderr.contains("missing") && !stderr.contains("corrupt"),
            "git fsck complained: {stderr}"
        );
        assert!(
            !String::from_utf8_lossy(&fsck.stdout).contains("error"),
            "git fsck stdout: {}",
            String::from_utf8_lossy(&fsck.stdout)
        );
    }

    // ----------------------------------------------------------- 3. 幂等/原子

    /// 验收 3：同一对象写两次不重写文件（inode/mtime/内容都不变），权限与 git 一致。
    #[test]
    fn writing_the_same_object_twice_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init(tmp.path(), crate::repo::DEFAULT_INITIAL_BRANCH).unwrap();

        let payload = b"idempotent payload\n".as_slice();
        let oid = write_loose(&repo, Kind::Blob, payload).unwrap();
        let path = loose_path(&repo, oid);
        let before = fs::metadata(&path).unwrap();
        let bytes_before = fs::read(&path).unwrap();

        assert_eq!(before.mode() & 0o7777, FILE_MODE, "loose file must be 0444");
        assert_eq!(
            fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o7777,
            DIR_MODE,
            "shard directory must be 0755"
        );
        assert!(temp_files(&repo).is_empty(), "temp file left behind");

        std::thread::sleep(std::time::Duration::from_millis(20));
        let again = write_loose(&repo, Kind::Blob, payload).unwrap();
        assert_eq!(again, oid, "same payload must hash to the same oid");

        let after = fs::metadata(&path).unwrap();
        assert_eq!(after.ino(), before.ino(), "object file was rewritten");
        assert_eq!(
            (after.mtime(), after.mtime_nsec()),
            (before.mtime(), before.mtime_nsec()),
            "object file mtime changed"
        );
        assert_eq!(fs::read(&path).unwrap(), bytes_before);
        assert!(temp_files(&repo).is_empty());

        // 另一个对象仍然能正常写入（写入路径没有被 0444 卡住）。
        let other = write_loose(&repo, Kind::Blob, b"another payload\n").unwrap();
        assert_ne!(other, oid);
        assert!(loose_path(&repo, other).is_file());
        assert!(temp_files(&repo).is_empty());
    }

    // -------------------------------------------------------------- 4. 遍历

    /// 验收 4：`iter_loose` 与 `git cat-file --batch-all-objects --batch-check` 的集合一致
    /// （本仓库没有 pack，所以 git 列出的全部对象都是 loose）。
    #[test]
    fn iter_loose_matches_git_batch_all_objects() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_git_repo(dir);
        fs::write(dir.join("b.txt"), "second\n").unwrap();
        git(dir, &["add", "b.txt"]);
        git(dir, &["commit", "-q", "-m", "second"]);
        git(dir, &["branch", "topic"]);
        git(dir, &["tag", "v1"]);
        git(dir, &["tag", "-a", "v2", "-m", "annotated"]);

        let repo = repo_at(dir);
        // mg 也写几个进去，两边必须仍然一致。
        write_loose(&repo, Kind::Blob, b"written by mg\n").unwrap();
        write_loose(&repo, Kind::Blob, b"also written by mg\n").unwrap();

        // 干扰项：都不是合法的 loose 对象名，必须被跳过。
        fs::create_dir_all(repo.objects_dir().join("zz")).unwrap();
        fs::write(repo.objects_dir().join("zz").join("0".repeat(38)), "junk").unwrap();
        fs::create_dir_all(repo.objects_dir().join("ab")).unwrap();
        fs::write(repo.objects_dir().join("ab/nothex"), "junk").unwrap();
        fs::write(
            repo.objects_dir().join("tmp_obj_leftover"),
            "junk temp file",
        )
        .unwrap();
        fs::write(repo.objects_dir().join("info").join("0".repeat(40)), "junk").unwrap();
        fs::write(repo.objects_dir().join("info").join("alternates"), "").unwrap();

        let mut expected: Vec<Oid> = git(
            dir,
            &[
                "cat-file",
                "--batch-all-objects",
                "--batch-check=%(objectname)",
            ],
        )
        .lines()
        .map(|line| oid(line.trim()))
        .collect();
        expected.sort();
        expected.dedup();

        let got = iter_loose(&repo).unwrap();
        assert!(!got.is_empty(), "no loose objects found at all");
        assert_eq!(got, expected, "iter_loose != git batch-all-objects");
        // 反向数量断言：多出任何条目（`^peeled` 式的幽灵对象、临时文件）都会在这里被抓到。
        assert_eq!(got.len(), expected.len());
        // 干扰项确实没有混进来。
        assert!(!got.contains(&oid(&"0".repeat(40))));
        assert!(!got.iter().any(|found| found.to_hex().starts_with("zz")));
    }

    // -------------------------------------------------------------- 5. 反例

    /// 验收 5：截断/篡改/缺对象/非法目录名——必须失败，而不是「安静地返回部分数据」。
    #[test]
    fn corrupt_objects_are_rejected_and_missing_ones_are_none() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init(tmp.path(), crate::repo::DEFAULT_INITIAL_BRANCH).unwrap();
        let payload = b"tamper me\n".as_slice();
        let oid = write_loose(&repo, Kind::Blob, payload).unwrap();
        let path = loose_path(&repo, oid);
        let good = fs::read(&path).unwrap();

        // (a) 截断的 zlib 流：整个半截、少 2 字节、少 1 字节（只丢 adler32 尾部）都必须报 Corrupt。
        for cut in [good.len() / 2, 2, 1] {
            fs::remove_file(&path).unwrap();
            fs::write(&path, &good[..good.len() - cut]).unwrap();
            let err = read_loose(&repo, oid).unwrap_err();
            assert!(
                matches!(err, Error::Corrupt { .. }),
                "truncated by {cut}: expected Corrupt, got {err:?}"
            );
            assert!(
                matches!(read_loose_raw(&repo, oid), Err(Error::Corrupt { .. })),
                "truncated by {cut}: read_loose_raw must be Corrupt too"
            );
        }

        // (b) payload 改一个字节但保留文件名：长度没变，只有内容哈希变了 → Corrupt。
        fs::remove_file(&path).unwrap();
        let tampered = object::encode(Kind::Blob, b"tamper ME\n");
        assert_eq!(tampered.len(), object::encode(Kind::Blob, payload).len());
        fs::write(&path, zlib::deflate(&tampered).unwrap()).unwrap();
        let err = read_loose(&repo, oid).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "tampered payload: expected Corrupt, got {err:?}"
        );
        // `Odb::verify` 的分工：raw 读取不判 oid，于是 fsck 得到 Ok(false) 而不是错误。
        assert!(!Odb::new(&repo).verify(oid).unwrap());
        assert!(read_loose_raw(&repo, oid).unwrap().is_some());

        // (c) header 里的长度与实际 payload 不符。
        fs::remove_file(&path).unwrap();
        fs::write(&path, zlib::deflate(b"blob 99\0short").unwrap()).unwrap();
        assert!(matches!(read_loose(&repo, oid), Err(Error::Corrupt { .. })));

        // (d) 根本不是 zlib 流。
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"not a zlib stream at all").unwrap();
        assert!(matches!(read_loose(&repo, oid), Err(Error::Corrupt { .. })));

        // (e) 未知类型名。
        fs::remove_file(&path).unwrap();
        fs::write(&path, zlib::deflate(b"widget 3\0abc").unwrap()).unwrap();
        assert!(matches!(read_loose(&repo, oid), Err(Error::Corrupt { .. })));

        // (f) 文件不存在 → Ok(None)（不是错误），raw 也一样。
        fs::remove_file(&path).unwrap();
        let missing = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();
        assert!(read_loose(&repo, missing).unwrap().is_none());
        assert!(read_loose_raw(&repo, missing).unwrap().is_none());
        assert!(!Odb::new(&repo).verify(missing).unwrap());
    }

    /// oid 与内容一致的对象被写坏后，`Odb::read` 必须把它当损坏而不是「找不到」。
    #[test]
    fn odb_read_reports_corruption_not_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init(tmp.path(), crate::repo::DEFAULT_INITIAL_BRANCH).unwrap();
        let oid = write_loose(&repo, Kind::Blob, b"boom\n").unwrap();
        let path = loose_path(&repo, oid);
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"garbage").unwrap();
        assert!(matches!(
            Odb::new(&repo).read(oid),
            Err(Error::Corrupt { .. })
        ));
    }
}
