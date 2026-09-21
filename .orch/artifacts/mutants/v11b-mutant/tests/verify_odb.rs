//! V5 —— 独立验证 T5（`src/odb/loose.rs` + `src/cli/cat_file.rs`）。
//!
//! 真值只来自运行时的真实 `git` 二进制与文件系统本身：本文件不出现任何手写的 oid、
//! 对象字节或期望输出；所有期望值都由 `git cat-file` / `git ls-tree` / `git rev-parse` /
//! `find` 现场产出。`git` 不可用时直接 panic —— 绝不静默跳过（防假绿）。
//!
//! 被测面：`minigit::odb::{Odb, loose}` 与 `mg` 二进制（`CARGO_BIN_EXE_mg`）。

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

use minigit::object::{self, Kind};
use minigit::odb::{loose, Odb};
use minigit::{Error, Oid, Repo};

// ---------------------------------------------------------------- git 测试床

fn require_git() {
    let ok = Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    assert!(
        ok,
        "this verification target requires the real git binary on PATH"
    );
}

/// 真值只能来自 git 二进制；身份/配置/日期全部就地隔离，绝不 export 进共享 shell。
fn git_raw(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "V5")
        .env("GIT_AUTHOR_EMAIL", "v5@example.com")
        .env("GIT_COMMITTER_NAME", "V5")
        .env("GIT_COMMITTER_EMAIL", "v5@example.com")
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

fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = git_raw(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn git_str(dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git_bytes(dir, args))
        .trim()
        .to_string()
}

/// `mg` 二进制：由 cargo 为集成测试注入。
fn mg_raw(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mg"))
        .current_dir(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .output()
        .expect("failed to spawn mg")
}

/// `git init` + 一个 commit（含一个子树和一个二进制文件）+ 一个 annotated tag。
fn git_fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    git_str(dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("a.txt"), "hello from git\n").expect("write a.txt");
    fs::create_dir_all(dir.join("sub")).expect("mkdir sub");
    fs::write(dir.join("sub/b.bin"), [0u8, 1, 2, 127, 255, 10]).expect("write b.bin");
    git_str(dir, &["add", "-A"]);
    git_str(dir, &["commit", "-q", "-m", "initial"]);
    git_str(dir, &["tag", "-a", "v-verify", "-m", "annotated"]);
    tmp
}

/// 手工编码一个 tree 载荷（不借用 T1 的 `Tree`）：`<mode> <name>\0<20B oid>` 顺序拼接。
/// 条目顺序由调用方保证是 git 的规范序。
fn raw_tree(entries: &[(&str, &[u8], Oid)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (mode, name, oid) in entries {
        out.extend_from_slice(mode.as_bytes());
        out.push(b' ');
        out.extend_from_slice(name);
        out.push(0);
        out.extend_from_slice(oid.as_bytes());
    }
    out
}

fn is_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// 文件系统真值：`find <objects> -type f` 里形状合法的 loose 对象（2 hex 目录 + 38 hex 文件）。
fn find_loose(objects: &Path) -> Vec<Oid> {
    let out = Command::new("find")
        .arg(objects)
        .args(["-type", "f"])
        .output()
        .expect("failed to spawn find");
    assert!(out.status.success(), "find failed: {}", out.status);
    let mut oids = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let path = Path::new(line);
        let Some(prefix) = path.parent().and_then(Path::file_name).and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if prefix.len() == 2 && name.len() == 38 && is_hex(prefix) && is_hex(name) {
            oids.push(Oid::from_hex(&format!("{prefix}{name}")).expect("legal loose path"));
        }
    }
    oids.sort();
    oids
}

/// `objects/` 顶层的 `tmp*` 残留（写入必须自己清理临时文件）。
fn top_level_tmp(objects: &Path) -> Vec<String> {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(objects) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("tmp") {
                found.push(name);
            }
        }
    }
    found
}

// (B)1 —— mg 写 → 真实 git 读 --------------------------------------------------

#[test]
fn mg_written_objects_are_readable_by_real_git() {
    require_git();
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    git_str(dir, &["init", "-q", "-b", "main"]);
    let repo = Repo::discover(dir).unwrap();
    let odb = Odb::new(&repo);

    // blob → 子树 → 顶层树 → commit → annotated tag，全部经 `Odb::write` 落盘。
    let inner_payload: &[u8] = b"inner payload\x00\xff\n";
    let inner = odb.write(Kind::Blob, inner_payload).unwrap();
    let inner_tree_payload = raw_tree(&[("100644", b"inner.txt", inner)]);
    let inner_tree = odb.write(Kind::Tree, &inner_tree_payload).unwrap();

    let file_payload: &[u8] = b"top-level\n";
    let file_blob = odb.write(Kind::Blob, file_payload).unwrap();
    let top_tree_payload = raw_tree(&[
        ("100644", b"a.txt", file_blob),
        ("40000", b"dir", inner_tree),
    ]);
    let top_tree = odb.write(Kind::Tree, &top_tree_payload).unwrap();

    let commit_payload = format!(
        "tree {top_tree}\nauthor V5 <v5@example.com> 1700000000 +0800\ncommitter V5 <v5@example.com> 1700000000 +0800\n\nwritten by mg\n"
    )
    .into_bytes();
    let commit = odb.write(Kind::Commit, &commit_payload).unwrap();

    let tag_payload = format!(
        "object {commit}\ntype commit\ntag v5-verify\ntagger V5 <v5@example.com> 1700000000 +0800\n\ntag written by mg\n"
    )
    .into_bytes();
    let tag = odb.write(Kind::Tag, &tag_payload).unwrap();

    // 真实 git 必须能读：类型 / 载荷长度 / 原始载荷逐字节一致。
    for (oid, expected_kind, payload) in [
        (inner, "blob", inner_payload),
        (inner_tree, "tree", inner_tree_payload.as_slice()),
        (file_blob, "blob", file_payload),
        (top_tree, "tree", top_tree_payload.as_slice()),
        (commit, "commit", commit_payload.as_slice()),
        (tag, "tag", tag_payload.as_slice()),
    ] {
        let hex = oid.to_hex();
        assert_eq!(git_str(dir, &["cat-file", "-t", &hex]), expected_kind, "{hex} type");
        assert_eq!(
            git_str(dir, &["cat-file", "-s", &hex]).parse::<usize>().unwrap(),
            payload.len(),
            "{hex} size"
        );
        assert_eq!(
            git_bytes(dir, &["cat-file", expected_kind, &hex]),
            payload,
            "{hex} raw payload read back by git differs"
        );
    }

    // tree 的 pretty 输出必须就是 git 自己的 `ls-tree`。
    for tree in [top_tree, inner_tree] {
        let hex = tree.to_hex();
        assert_eq!(
            git_bytes(dir, &["cat-file", "-p", &hex]),
            git_bytes(dir, &["ls-tree", &hex]),
            "git's own -p and ls-tree disagree on {hex}"
        );
    }

    // `git fsck` 不得报 error / missing / corrupt（dangling 提示不算）。
    let fsck = git_raw(dir, &["fsck", "--no-progress"]);
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&fsck.stdout),
        String::from_utf8_lossy(&fsck.stderr)
    );
    assert!(fsck.status.success(), "git fsck failed: {report}");
    for bad in ["error", "missing", "corrupt"] {
        assert!(
            !report.to_ascii_lowercase().contains(bad),
            "git fsck reported {bad:?}: {report}"
        );
    }
}

// (B)2 —— git 写 → mg 读 ------------------------------------------------------

#[test]
fn git_written_objects_are_read_by_minigit() {
    require_git();
    let tmp = git_fixture();
    let dir = tmp.path();
    let repo = Repo::discover(dir).unwrap();
    let odb = Odb::new(&repo);

    for rev in [
        "HEAD:a.txt",
        "HEAD:sub/b.bin",
        "HEAD^{tree}",
        "HEAD:sub",
        "HEAD",
        "refs/tags/v-verify",
    ] {
        let hex = git_str(dir, &["rev-parse", rev]);
        let oid = Oid::from_hex(hex.trim()).unwrap();
        let type_name = git_str(dir, &["cat-file", "-t", &hex]);
        let expected_kind = match type_name.as_str() {
            "blob" => Kind::Blob,
            "tree" => Kind::Tree,
            "commit" => Kind::Commit,
            "tag" => Kind::Tag,
            other => panic!("unexpected git object type {other:?} for {rev}"),
        };
        let size: usize = git_str(dir, &["cat-file", "-s", &hex]).parse().unwrap();
        let raw_payload = git_bytes(dir, &["cat-file", &type_name, &hex]);

        let (kind, payload) = odb.read(oid).unwrap();
        assert_eq!(kind, expected_kind, "{rev}: kind");
        assert_eq!(payload.len(), size, "{rev}: size vs git cat-file -s");
        assert_eq!(payload, raw_payload, "{rev}: payload vs git cat-file <type>");

        // `read_loose_raw` 必须是 git 的 `<type> <size>\0` + payload 形态。
        let raw = loose::read_loose_raw(&repo, oid)
            .unwrap()
            .expect("object exists");
        let mut expected_raw = format!("{type_name} {size}\0").into_bytes();
        expected_raw.extend_from_slice(&raw_payload);
        assert_eq!(raw, expected_raw, "{rev}: loose raw bytes");
        assert_eq!(raw, object::encode(expected_kind, &payload), "{rev}: encode(raw)");
    }

    // 不存在的 oid：`Odb::read` 必须是带类型的 ObjectNotFound，而不是 panic。
    let absent = Oid::hash_object("blob", b"v5-absent-object");
    assert!(
        matches!(odb.read(absent), Err(Error::ObjectNotFound(_))),
        "absent oid must be Error::ObjectNotFound"
    );
}

// (B)3 —— CLI 端到端 ----------------------------------------------------------

#[test]
fn mg_cat_file_matches_real_git_byte_for_byte() {
    require_git();
    let tmp = git_fixture();
    let dir = tmp.path();

    let blob = git_str(dir, &["rev-parse", "HEAD:a.txt"]);
    let bin_blob = git_str(dir, &["rev-parse", "HEAD:sub/b.bin"]);
    let tree = git_str(dir, &["rev-parse", "HEAD^{tree}"]);
    let subtree = git_str(dir, &["rev-parse", "HEAD:sub"]);
    let commit = git_str(dir, &["rev-parse", "HEAD"]);
    let tag = git_str(dir, &["rev-parse", "refs/tags/v-verify"]);

    for oid in [&blob, &bin_blob, &tree, &subtree, &commit, &tag] {
        for flag in ["-t", "-s", "-p"] {
            let ours = mg_raw(dir, &["cat-file", flag, oid]);
            assert!(
                ours.status.success(),
                "mg cat-file {flag} {oid} failed: {}",
                String::from_utf8_lossy(&ours.stderr)
            );
            let theirs = git_raw(dir, &["cat-file", flag, oid]);
            assert!(theirs.status.success(), "git cat-file {flag} {oid} failed");
            assert_eq!(
                ours.stdout, theirs.stdout,
                "`mg cat-file {flag} {oid}` != `git cat-file {flag} {oid}`"
            );
            assert!(ours.stderr.is_empty(), "mg wrote to stderr: {:?}", ours.stderr);
        }
    }

    // tree 的 -p 还必须与 `git ls-tree` 逐字节一致（含 tab 与换行）。
    for oid in [&tree, &subtree] {
        let ours = mg_raw(dir, &["cat-file", "-p", oid]);
        assert!(ours.status.success());
        assert_eq!(
            ours.stdout,
            git_bytes(dir, &["ls-tree", oid]),
            "`mg cat-file -p {oid}` != `git ls-tree {oid}`"
        );
    }

    // -e：存在 → exit 0 且零输出；不存在 → 非零 + stderr 有信息 + 不 panic。
    let present = mg_raw(dir, &["cat-file", "-e", &blob]);
    assert!(present.status.success(), "-e on an existing object must exit 0");
    assert!(present.stdout.is_empty() && present.stderr.is_empty());

    let absent = Oid::hash_object("blob", b"v5-cli-absent").to_hex();
    let missing = mg_raw(dir, &["cat-file", "-e", &absent]);
    assert!(!missing.status.success(), "-e on a missing object must be non-zero");
    assert!(missing.stdout.is_empty());
    assert!(!missing.stderr.is_empty(), "-e must explain on stderr");

    let missing_p = mg_raw(dir, &["cat-file", "-p", &absent]);
    assert!(!missing_p.status.success(), "-p on a missing object must be non-zero");
    assert!(missing_p.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&missing_p.stderr);
    assert!(!stderr.trim().is_empty(), "-p must explain on stderr");
    assert!(
        !stderr.to_ascii_lowercase().contains("panic"),
        "must not panic: {stderr}"
    );
}

// (B)4 —— 幂等与原子性 --------------------------------------------------------

#[test]
fn writing_is_idempotent_atomic_and_leaves_no_temp_files() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repo::init(tmp.path(), "main").unwrap();
    let odb = Odb::new(&repo);

    let payload: &[u8] = b"idempotent payload\n\x00\xff";
    let first = odb.write(Kind::Blob, payload).unwrap();
    assert_eq!(first, Oid::hash_object("blob", payload), "write must return content hash");

    let path = repo.objects_dir().join(first.loose_rel_path());
    assert!(path.is_file(), "object file missing at {}", path.display());
    let shard_mode = fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
    assert_eq!(shard_mode, 0o755, "shard dir must be 0755");
    let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o444, "loose object must be 0444");
    assert!(
        top_level_tmp(&repo.objects_dir()).is_empty(),
        "temp residue after first write: {:?}",
        top_level_tmp(&repo.objects_dir())
    );

    let before = fs::metadata(&path).unwrap();
    let bytes_before = fs::read(&path).unwrap();
    thread::sleep(Duration::from_millis(30));
    let second = odb.write(Kind::Blob, payload).unwrap();
    assert_eq!(second, first, "same payload must map to the same oid");

    let after = fs::metadata(&path).unwrap();
    assert_eq!(after.ino(), before.ino(), "existing object was rewritten (inode changed)");
    assert_eq!(
        (after.mtime(), after.mtime_nsec()),
        (before.mtime(), before.mtime_nsec()),
        "existing object was rewritten (mtime changed)"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes_before, "object bytes changed");
    assert!(
        top_level_tmp(&repo.objects_dir()).is_empty(),
        "temp residue after second write: {:?}",
        top_level_tmp(&repo.objects_dir())
    );

    // 0444 不会卡住后续写：另一个对象仍能落盘。
    let other = odb.write(Kind::Blob, b"a different payload\n").unwrap();
    assert_ne!(other, first);
    assert!(repo.objects_dir().join(other.loose_rel_path()).is_file());
    assert!(top_level_tmp(&repo.objects_dir()).is_empty());
}

// (B)5 —— iter_loose -----------------------------------------------------------

#[test]
fn iter_loose_matches_the_filesystem_loose_set() {
    require_git();
    let tmp = git_fixture();
    let dir = tmp.path();
    let repo = Repo::discover(dir).unwrap();
    let odb = Odb::new(&repo);
    odb.write(Kind::Blob, b"mg iter one\n").unwrap();
    odb.write(Kind::Blob, b"mg iter two\n").unwrap();

    // 干扰项：非法分片目录名 / 非法文件名 / 顶层临时文件。
    let objects = repo.objects_dir();
    fs::create_dir_all(objects.join("zz")).unwrap();
    fs::write(objects.join("zz").join("0".repeat(38)), b"junk").unwrap();
    fs::create_dir_all(objects.join("ab")).unwrap();
    fs::write(objects.join("ab/nothex"), b"junk").unwrap();
    fs::write(objects.join("tmp_obj_leftover"), b"junk").unwrap();

    let expected = find_loose(&dir.join(".git/objects"));
    assert!(!expected.is_empty(), "fixture produced no loose objects");
    let got = odb.iter_loose().unwrap();
    assert_eq!(got, expected, "iter_loose != `find .git/objects -type f` legal set");
    assert!(
        got.windows(2).all(|pair| pair[0] < pair[1]),
        "iter_loose must be strictly sorted and deduped"
    );
    assert!(!got.iter().any(|oid| oid.to_hex().starts_with("zz")));
}

// (B)6 —— 反例（必须失败） -----------------------------------------------------

#[test]
fn corrupt_and_missing_objects_are_typed_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repo::init(tmp.path(), "main").unwrap();
    let odb = Odb::new(&repo);

    let payload: &[u8] = b"tamper target\n";
    let oid = odb.write(Kind::Blob, payload).unwrap();
    let path = repo.objects_dir().join(oid.loose_rel_path());
    let good = fs::read(&path).unwrap();
    assert!(good.len() > 2);

    // (a) 截断的 zlib 流 → Corrupt（read_loose 与 read_loose_raw 都要）。
    // 对象文件是 0444 只读，改写前必须先 unlink。
    fs::remove_file(&path).unwrap();
    fs::write(&path, &good[..good.len() - 2]).unwrap();
    assert!(matches!(
        loose::read_loose(&repo, oid),
        Err(Error::Corrupt { .. })
    ));
    assert!(matches!(
        loose::read_loose_raw(&repo, oid),
        Err(Error::Corrupt { .. })
    ));

    // (b) payload 改一个字节、保留文件名（重新压成合法 zlib）→ Corrupt（oid 校验生效）。
    let mut mutated = object::encode(Kind::Blob, payload);
    let last = mutated.len() - 1;
    mutated[last] ^= 0x01;
    fs::remove_file(&path).unwrap();
    fs::write(&path, minigit::zlib::deflate(&mutated).unwrap()).unwrap();
    let err = loose::read_loose(&repo, oid).unwrap_err();
    assert!(
        matches!(err, Error::Corrupt { .. }),
        "tampered payload must be Corrupt, got {err:?}"
    );

    // (c) 根本不是 zlib 流 → Corrupt。
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"certainly not a zlib stream").unwrap();
    assert!(matches!(
        loose::read_loose(&repo, oid),
        Err(Error::Corrupt { .. })
    ));

    // (d) 不存在的 oid → Ok(None)（不是错误）。
    fs::remove_file(&path).unwrap();
    let absent = Oid::hash_object("blob", b"v5-definitely-absent");
    assert!(loose::read_loose(&repo, absent).unwrap().is_none());
    assert!(loose::read_loose_raw(&repo, absent).unwrap().is_none());

    // (e) CLI：不存在的 oid → 非零退出 + stderr 有信息，且绝不 panic。
    let out = mg_raw(tmp.path(), &["cat-file", "-p", &absent.to_hex()]);
    assert!(!out.status.success(), "mg cat-file -p <missing> must be non-zero");
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.trim().is_empty(), "stderr must carry a message");
    assert!(
        !stderr.to_ascii_lowercase().contains("panic"),
        "must not panic: {stderr}"
    );
}
