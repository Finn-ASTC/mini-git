//! V3 — independent verification of T3 (`src/index/dirc.rs`, DIRC v2 read/write).
//!
//! Truth source: a real `git` binary running against an isolated `tempfile::tempdir()`
//! repository. This target hardcodes no index bytes and does not copy the implementation
//! under test; every roundtrip assertion is "git writes -> minigit reads -> minigit
//! writes -> byte-compare against what git wrote".

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use sha1::{Digest, Sha1};

use minigit::error::Error;
use minigit::index::Index;
use minigit::repo::Repo;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// One `git ls-files --stage -z` record, parsed from git's own output.
#[derive(Clone, Debug, PartialEq, Eq)]
struct GitEntry {
    mode: String,
    oid: String,
    stage: u8,
    path: Vec<u8>,
}

struct Scratch {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Scratch {
        let prefix = format!(
            "vx3-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        );
        let tmp = tempfile::Builder::new()
            .prefix(&prefix)
            .tempdir()
            .expect("tempdir");
        let dir = tmp.path().to_path_buf();
        let scratch = Scratch { _tmp: tmp, dir };
        scratch.ok(&["-c", "init.defaultBranch=main", "init", "-q"]);
        scratch.ok(&["config", "user.name", "V3"]);
        scratch.ok(&["config", "user.email", "v3@example.com"]);
        scratch
    }

    fn git(&self, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "V3")
            .env("GIT_AUTHOR_EMAIL", "v3@example.com")
            .env("GIT_COMMITTER_NAME", "V3")
            .env("GIT_COMMITTER_EMAIL", "v3@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0000")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("failed to spawn git")
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.git(args);
        assert!(
            out.status.success(),
            "git {args:?} failed (exit {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim_end().to_string()
    }

    fn bash(&self, script: &str) -> Output {
        Command::new("bash")
            .arg("-c")
            .arg(script)
            .current_dir(&self.dir)
            .output()
            .expect("failed to spawn bash")
    }

    fn repo(&self) -> Repo {
        Repo::discover(&self.dir).expect("Repo::discover")
    }

    fn index_bytes(&self) -> Vec<u8> {
        fs::read(self.dir.join(".git/index")).expect("read .git/index")
    }

    fn put_index_bytes(&self, bytes: &[u8]) {
        fs::write(self.dir.join(".git/index"), bytes).expect("write .git/index");
    }

    fn ls_stage_z(&self) -> Vec<u8> {
        self.git(&["ls-files", "--stage", "-z"]).stdout
    }
}

fn parse_stage_z(raw: &[u8]) -> Vec<GitEntry> {
    let mut entries = Vec::new();
    for record in raw.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .expect("ls-files --stage record must contain a tab");
        let meta = std::str::from_utf8(&record[..tab]).expect("meta is ascii");
        let mut fields = meta.split(' ');
        let mode = fields.next().expect("mode").to_string();
        let oid = fields.next().expect("oid").to_string();
        let stage = fields.next().expect("stage").parse::<u8>().expect("stage");
        entries.push(GitEntry {
            mode,
            oid,
            stage,
            path: record[tab + 1..].to_vec(),
        });
    }
    entries
}

fn minigit_entries(index: &Index) -> Vec<GitEntry> {
    index
        .entries
        .iter()
        .map(|entry| GitEntry {
            mode: entry.mode.as_str().to_string(),
            oid: entry.oid.to_hex(),
            stage: entry.stage,
            path: entry.path.clone(),
        })
        .collect()
}

fn check_entries_match(index: &Index, git_entries: &[GitEntry]) {
    let mine = minigit_entries(index);
    assert_eq!(
        mine.len(),
        git_entries.len(),
        "entry count differs: minigit={} git={}",
        mine.len(),
        git_entries.len()
    );
    for (ordinal, (a, b)) in mine.iter().zip(git_entries).enumerate() {
        let path_diff = a.path.iter().zip(&b.path).position(|(x, y)| x != y);
        assert!(
            a == b,
            "entry {ordinal} differs at path byte {path_diff:?}: minigit(mode={} stage={} len={}) git(mode={} stage={} len={})",
            a.mode,
            a.stage,
            a.path.len(),
            b.mode,
            b.stage,
            b.path.len()
        );
    }
}

fn assert_bytes_eq(got: &[u8], want: &[u8], what: &str) {
    if got != want {
        let at = got
            .iter()
            .zip(want)
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| got.len().min(want.len()));
        panic!(
            "{what}: bytes differ (got {} bytes, want {} bytes, first differing offset {at})",
            got.len(),
            want.len()
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// A `/`-separated relative path of exactly `total` bytes (components <= 250 bytes).
fn long_path(total: usize) -> String {
    let width = 250usize;
    let mut parts: Vec<String> = Vec::new();
    let mut joined_len = 0usize;
    while total - joined_len - 1 > 255 {
        parts.push("a".repeat(width));
        joined_len += if parts.len() == 1 { width } else { width + 1 };
    }
    let last = total - joined_len - 1;
    parts.push("b".repeat(last));
    let path = parts.join("/");
    assert_eq!(path.len(), total, "long_path must be exact");
    path
}

fn build_diverse_worktree(s: &Scratch) {
    fs::create_dir_all(s.dir.join("nested/dir")).unwrap();
    fs::write(s.dir.join("nested/dir/file.txt"), b"hello\n").unwrap();
    fs::write(s.dir.join("exec.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    let mut perms = fs::metadata(s.dir.join("exec.sh")).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(s.dir.join("exec.sh"), perms).unwrap();
    symlink("nested/dir/file.txt", s.dir.join("link")).unwrap();
    fs::write(s.dir.join("中文文件.txt"), b"utf8\n").unwrap();
    // Cover every padding residue of (62 + path_len) mod 8, including the 8-NUL case.
    for n in 1..=9usize {
        let name = "p".repeat(n);
        fs::write(s.dir.join(&name), format!("{n}\n").as_bytes()).unwrap();
    }
    let deep = long_path(4095);
    let parent = deep.rsplit_once('/').expect("deep path has a parent").0;
    let script = format!("mkdir -p -- '{parent}' && printf deep > '{deep}'");
    let out = s.bash(&script);
    assert!(
        out.status.success(),
        "creating the 4095-byte path failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn git_written_diverse_index_roundtrips_byte_for_byte() {
    let s = Scratch::new("diverse");
    build_diverse_worktree(&s);
    s.ok(&["add", "-A"]);
    // bit15 of the flags word must survive as `assume_valid`.
    s.ok(&["update-index", "--assume-unchanged", "exec.sh"]);

    let before_index = s.index_bytes();
    let before_ls = s.ls_stage_z();
    let git_entries = parse_stage_z(&before_ls);

    assert!(
        git_entries.iter().any(|entry| entry.path.len() == 4095),
        "git must record the 4095-byte deep path, got lengths {:?}",
        git_entries.iter().map(|e| e.path.len()).collect::<Vec<_>>()
    );
    assert!(
        git_entries.iter().any(|entry| entry.mode == "100755"),
        "exec bit"
    );
    assert!(
        git_entries.iter().any(|entry| entry.mode == "120000"),
        "symlink"
    );
    assert!(
        git_entries
            .iter()
            .any(|entry| entry.path == "中文文件.txt".as_bytes()),
        "utf-8 path"
    );

    let repo = s.repo();

    // Roundtrip 1: staged-only index (no TREE extension yet).
    let index = Index::read(&repo).expect("Index::read on git's index");
    check_entries_match(&index, &git_entries);
    assert!(
        index.tree_oid.is_none(),
        "`git add` alone must not emit a TREE extension"
    );
    assert!(
        index
            .lookup(b"exec.sh")
            .expect("exec.sh entry")
            .assume_valid,
        "assume-valid bit must be parsed"
    );
    index.write(&repo).expect("Index::write");
    assert_bytes_eq(
        &s.index_bytes(),
        &before_index,
        "index byte roundtrip (staged-only)",
    );
    assert_bytes_eq(
        &s.ls_stage_z(),
        &before_ls,
        "ls-files --stage after roundtrip",
    );
    s.ok(&["update-index", "--refresh"]);
    s.ok(&["fsck", "--no-progress"]);

    // Roundtrip 2: committed index (carries an extra TREE cache-tree) must still
    // roundtrip, and real git must still see a clean worktree afterwards.
    s.ok(&["commit", "-q", "-m", "init"]);
    let committed_index = s.index_bytes();
    let committed_ls = s.ls_stage_z();
    let status_before = s.git(&["status", "--porcelain"]);
    assert!(status_before.status.success(), "git status failed");
    assert_bytes_eq(
        &status_before.stdout,
        b"",
        "git status --porcelain before roundtrip",
    );

    let index = Index::read(&repo).expect("Index::read on the committed index");
    check_entries_match(&index, &parse_stage_z(&committed_ls));
    index.write(&repo).expect("Index::write");
    assert_bytes_eq(
        &s.index_bytes(),
        &committed_index,
        "index byte roundtrip (committed)",
    );
    assert_bytes_eq(
        &s.ls_stage_z(),
        &committed_ls,
        "ls-files --stage after committed roundtrip",
    );

    let status_after = s.git(&["status", "--porcelain"]);
    assert!(
        status_after.status.success(),
        "git status failed after roundtrip"
    );
    assert_bytes_eq(
        &status_after.stdout,
        b"",
        "git status --porcelain after roundtrip",
    );
    s.ok(&["update-index", "--refresh"]);
}

#[test]
fn committed_index_with_tree_extension_roundtrips_and_exposes_tree_oid() {
    let s = Scratch::new("tree");
    fs::write(s.dir.join("a.txt"), b"a\n").unwrap();
    fs::create_dir_all(s.dir.join("sub")).unwrap();
    fs::write(s.dir.join("sub/b.txt"), b"b\n").unwrap();
    s.ok(&["add", "-A"]);
    s.ok(&["commit", "-q", "-m", "init"]);

    let before_index = s.index_bytes();
    let before_ls = s.ls_stage_z();
    assert!(
        contains(&before_index, b"TREE"),
        "git's committed index should carry a TREE cache-tree extension"
    );

    let repo = s.repo();
    let index = Index::read(&repo).expect("Index::read");
    assert!(
        index.extensions.iter().any(|ext| &ext.signature == b"TREE"),
        "TREE extension must be exposed"
    );
    let head_tree = s.ok(&["rev-parse", "HEAD^{tree}"]);
    let parsed = index
        .tree_oid
        .expect("tree_oid must be parsed from TREE")
        .to_hex();
    assert_eq!(
        parsed, head_tree,
        "parsed tree_oid must equal git's HEAD tree"
    );

    index.write(&repo).expect("Index::write");
    assert_bytes_eq(
        &s.index_bytes(),
        &before_index,
        "index byte roundtrip (TREE)",
    );
    assert_bytes_eq(
        &s.ls_stage_z(),
        &before_ls,
        "ls-files --stage after roundtrip (TREE)",
    );
    let status = s.git(&["status", "--porcelain"]);
    assert_bytes_eq(
        &status.stdout,
        b"",
        "git status --porcelain after roundtrip (TREE)",
    );
    s.ok(&["update-index", "--refresh"]);
}

#[test]
fn name_length_boundaries_4095_and_4096_roundtrip() {
    let s = Scratch::new("names");
    fs::write(s.dir.join("blob.txt"), b"x\n").unwrap();
    let oid = s.ok(&["hash-object", "-w", "blob.txt"]);

    // git itself writes these index entries (no worktree stat is possible at 4096 bytes).
    for total in [4094usize, 4095, 4096] {
        let path = long_path(total);
        assert_eq!(path.len(), total);
        s.ok(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{oid},{path}"),
        ]);
    }

    let before_index = s.index_bytes();
    let before_ls = s.ls_stage_z();
    let git_entries = parse_stage_z(&before_ls);
    let lengths: BTreeSet<usize> = git_entries.iter().map(|entry| entry.path.len()).collect();
    assert_eq!(
        lengths,
        BTreeSet::from([4094usize, 4095, 4096]),
        "git must store exact 4094/4095/4096 byte names"
    );

    let repo = s.repo();
    let index = Index::read(&repo).expect("Index::read with long names");
    check_entries_match(&index, &git_entries);

    index.write(&repo).expect("Index::write with long names");
    assert_bytes_eq(
        &s.index_bytes(),
        &before_index,
        "index byte roundtrip (long names)",
    );
    assert_bytes_eq(
        &s.ls_stage_z(),
        &before_ls,
        "ls-files --stage after roundtrip (long names)",
    );
}

#[test]
fn corrupt_indexes_are_rejected_without_panicking() {
    let s = Scratch::new("corrupt");
    fs::write(s.dir.join("a.txt"), b"a\n").unwrap();
    s.ok(&["add", "-A"]);
    let valid = s.index_bytes();
    let repo = s.repo();
    assert!(valid.len() > 64);

    // 1. flip the last byte (part of the trailer) -> Corrupt
    let mut flipped = valid.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0xff;
    s.put_index_bytes(&flipped);
    match Index::read(&repo) {
        Err(Error::Corrupt { .. }) => {}
        other => panic!("flipped last byte: expected Error::Corrupt, got {other:?}"),
    }

    // 2. flip a byte in the middle of the entry region -> Corrupt
    let mut middle = valid.clone();
    middle[40] ^= 0x01;
    s.put_index_bytes(&middle);
    match Index::read(&repo) {
        Err(Error::Corrupt { .. }) => {}
        other => panic!("flipped entry byte: expected Error::Corrupt, got {other:?}"),
    }

    // 3. version field = 3 -> Unsupported
    let mut version3 = valid.clone();
    version3[4..8].copy_from_slice(&3u32.to_be_bytes());
    s.put_index_bytes(&version3);
    match Index::read(&repo) {
        Err(Error::Unsupported(_)) => {}
        other => panic!("version 3: expected Error::Unsupported, got {other:?}"),
    }

    // 4. entry_count larger than the real entry data -> Err, no panic
    let mut inflated = valid.clone();
    inflated[8..12].copy_from_slice(&9999u32.to_be_bytes());
    s.put_index_bytes(&inflated);
    assert!(
        Index::read(&repo).is_err(),
        "inflated entry_count must error"
    );

    // 5. truncations -> Err (never panic)
    for len in [4usize, 12, 31] {
        s.put_index_bytes(&valid[..len]);
        assert!(
            Index::read(&repo).is_err(),
            "truncated to {len} bytes must error"
        );
    }
}

#[test]
fn real_git_index_version_3_is_reported_unsupported() {
    let s = Scratch::new("v3");
    fs::write(s.dir.join("a.txt"), b"a\n").unwrap();
    s.ok(&["add", "-A"]);
    // `--skip-worktree` makes real git upgrade the on-disk index to version 3.
    s.ok(&["update-index", "--skip-worktree", "a.txt"]);

    let bytes = s.index_bytes();
    let version = u32::from_be_bytes(bytes[4..8].try_into().expect("4 version bytes"));
    assert_eq!(version, 3, "git must have written a v3 index for this case");

    let repo = s.repo();
    match Index::read(&repo) {
        Err(Error::Unsupported(_)) => {}
        other => panic!("a real v3 index must yield Error::Unsupported, got {other:?}"),
    }
}

#[test]
fn missing_index_reads_empty_and_written_empty_index_is_git_readable() {
    let s = Scratch::new("empty");
    assert!(
        !s.dir.join(".git/index").exists(),
        "git init must not create an index"
    );

    let repo = s.repo();
    let index = Index::read(&repo).expect("missing index must not be an error");
    assert!(index.entries.is_empty(), "missing index must read as empty");

    index.write(&repo).expect("Index::write of the empty index");
    assert!(s.dir.join(".git/index").exists());

    assert_bytes_eq(
        &s.ls_stage_z(),
        b"",
        "git ls-files --stage on the written empty index",
    );
    assert_eq!(s.ok(&["ls-files", "--stage"]), "");
    let status = s.git(&["status", "--porcelain"]);
    assert!(status.status.success(), "git status failed on empty index");
    assert_bytes_eq(
        &status.stdout,
        b"",
        "git status --porcelain on the written empty index",
    );
    s.ok(&["update-index", "--refresh"]);
}

#[test]
fn conflicted_index_stages_roundtrip() {
    let s = Scratch::new("conflict");
    fs::write(s.dir.join("f.txt"), b"base\n").unwrap();
    s.ok(&["add", "-A"]);
    s.ok(&["commit", "-q", "-m", "base"]);
    s.ok(&["checkout", "-q", "-b", "side"]);
    fs::write(s.dir.join("f.txt"), b"side\n").unwrap();
    s.ok(&["commit", "-q", "-am", "side"]);
    s.ok(&["checkout", "-q", "main"]);
    fs::write(s.dir.join("f.txt"), b"main\n").unwrap();
    s.ok(&["commit", "-q", "-am", "main"]);

    let merge = s.git(&["merge", "side"]);
    assert!(!merge.status.success(), "merge must conflict for this test");

    let before_index = s.index_bytes();
    let before_ls = s.ls_stage_z();
    let git_entries = parse_stage_z(&before_ls);
    let stages: BTreeSet<u8> = git_entries.iter().map(|entry| entry.stage).collect();
    assert!(
        stages.contains(&1) && stages.contains(&2) && stages.contains(&3),
        "conflicted index must expose stages 1/2/3, got {stages:?}"
    );

    let repo = s.repo();
    let index = Index::read(&repo).expect("Index::read of a conflicted index");
    check_entries_match(&index, &git_entries);

    index
        .write(&repo)
        .expect("Index::write of a conflicted index");
    assert_bytes_eq(
        &s.index_bytes(),
        &before_index,
        "conflicted index byte roundtrip",
    );
    assert_bytes_eq(
        &s.ls_stage_z(),
        &before_ls,
        "ls-files --stage after conflicted roundtrip",
    );
}

#[test]
fn unknown_extension_next_to_tree_is_preserved_verbatim() {
    let s = Scratch::new("ext");
    fs::write(s.dir.join("a.txt"), b"a\n").unwrap();
    s.ok(&["add", "-A"]);
    s.ok(&["commit", "-q", "-m", "init"]);

    let original = s.index_bytes();
    assert!(
        contains(&original, b"TREE"),
        "committed index must carry a TREE extension"
    );

    // Append an extension real git does not know, then fix up the SHA-1 trailer.
    let payload: &[u8] = b"v3-verifier-unknown-extension-payload";
    let mut crafted = original[..original.len() - 20].to_vec();
    crafted.extend_from_slice(b"ZZzz");
    crafted.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    crafted.extend_from_slice(payload);
    let mut hasher = Sha1::new();
    hasher.update(&crafted);
    crafted.extend_from_slice(&hasher.finalize());
    s.put_index_bytes(&crafted);

    // git (the truth) still reads the crafted index.
    let ls_before = s.ls_stage_z();
    assert_eq!(parse_stage_z(&ls_before).len(), 1);

    let repo = s.repo();
    let index = Index::read(&repo).expect("Index::read with an unknown extension");
    let ext = index
        .extensions
        .iter()
        .find(|ext| &ext.signature == b"ZZzz")
        .expect("unknown extension must be kept in memory");
    assert_eq!(ext.data.as_slice(), payload, "unknown extension payload");
    assert!(index.tree_oid.is_some(), "TREE must still be parsed");
    let order: Vec<[u8; 4]> = index.extensions.iter().map(|ext| ext.signature).collect();
    assert_eq!(
        order.first(),
        Some(b"TREE"),
        "TREE must stay first: {order:?}"
    );

    index
        .write(&repo)
        .expect("Index::write with an unknown extension");
    assert_bytes_eq(
        &s.index_bytes(),
        &crafted,
        "unknown extension byte roundtrip",
    );
    assert_bytes_eq(
        &s.ls_stage_z(),
        &ls_before,
        "ls-files after unknown-extension roundtrip",
    );
}

#[test]
fn index_with_git_written_eoie_extension_roundtrips_byte_for_byte() {
    let s = Scratch::new("eoie");
    fs::write(s.dir.join("a.txt"), b"x\n").unwrap();
    fs::write(s.dir.join("b.txt"), b"y\n").unwrap();
    s.ok(&[
        "-c",
        "index.recordEndOfIndexEntries=true",
        "-c",
        "index.recordOffsetTable=true",
        "add",
        "-A",
    ]);

    let before_index = s.index_bytes();
    assert!(
        contains(&before_index, b"EOIE"),
        "git must have written an EOIE extension for this case"
    );
    let before_ls = s.ls_stage_z();

    let repo = s.repo();
    let index = Index::read(&repo).expect("Index::read with EOIE");
    assert!(
        index.extensions.iter().any(|ext| &ext.signature == b"EOIE"),
        "EOIE must be kept in memory"
    );
    index.write(&repo).expect("Index::write with EOIE");
    assert_bytes_eq(&s.index_bytes(), &before_index, "EOIE index byte roundtrip");
    assert_bytes_eq(&s.ls_stage_z(), &before_ls, "ls-files after EOIE roundtrip");
    assert!(
        !s.dir.join(".git/index.lock").exists(),
        "atomic write must not leave index.lock behind"
    );
    s.ok(&["update-index", "--refresh"]);
    s.ok(&["status", "--porcelain"]);
}

#[test]
fn index_with_git_written_untr_extension_roundtrips_byte_for_byte() {
    let s = Scratch::new("untr");
    fs::create_dir_all(s.dir.join("d")).unwrap();
    fs::write(s.dir.join("d/a.txt"), b"x\n").unwrap();
    fs::write(s.dir.join("b.txt"), b"y\n").unwrap();
    s.ok(&["add", "-A"]);
    s.ok(&[
        "-c",
        "core.untrackedCache=true",
        "update-index",
        "--untracked-cache",
    ]);

    let before_index = s.index_bytes();
    assert!(
        contains(&before_index, b"UNTR"),
        "git must have written a UNTR extension for this case"
    );
    let before_ls = s.ls_stage_z();

    let repo = s.repo();
    let index = Index::read(&repo).expect("Index::read with UNTR");
    assert!(
        index.extensions.iter().any(|ext| &ext.signature == b"UNTR"),
        "UNTR must be kept in memory"
    );
    index.write(&repo).expect("Index::write with UNTR");
    assert_bytes_eq(&s.index_bytes(), &before_index, "UNTR index byte roundtrip");
    assert_bytes_eq(&s.ls_stage_z(), &before_ls, "ls-files after UNTR roundtrip");
    s.ok(&["update-index", "--refresh"]);
    s.ok(&["status", "--porcelain"]);
}

#[test]
fn git_empty_index_with_tree_extension_roundtrips() {
    let s = Scratch::new("empty-tree");
    fs::write(s.dir.join("a.txt"), b"a\n").unwrap();
    s.ok(&["add", "-A"]);
    s.ok(&["read-tree", "--empty"]);
    let empty_tree = s.ok(&["write-tree"]);

    let before_index = s.index_bytes();
    let before_ls = s.ls_stage_z();
    assert!(
        before_ls.is_empty(),
        "read-tree --empty must clear the index"
    );

    let repo = s.repo();
    let index = Index::read(&repo).expect("Index::read of git's empty index");
    assert!(index.entries.is_empty(), "entry count must be zero");
    assert_eq!(
        index.tree_oid.expect("TREE of the empty index").to_hex(),
        empty_tree,
        "tree_oid must equal git write-tree's empty tree"
    );

    index.write(&repo).expect("Index::write of the empty index");
    assert_bytes_eq(
        &s.index_bytes(),
        &before_index,
        "empty index byte roundtrip",
    );
    assert_bytes_eq(
        &s.ls_stage_z(),
        &before_ls,
        "ls-files after empty-index roundtrip",
    );
    s.ok(&["update-index", "--refresh"]);
}
