//! V15 —— T15（`mg fsck` / `mg gc`）的**独立验证**。
//!
//! 真值来源：**真实 `git` 二进制**。本文件不引用 `mg` 自己算出的哈希/合法性作为证据：
//! `mg gc` 写出的 pack 一律交给 `git verify-pack` / `git fsck` / `git cat-file` /
//! `git log --all` 判定；`mg fsck` 的每个结论都与同一夹具上的真实 `git fsck` 对拍。
//!
//! 夹具状态在断言里写全（对象库内容、loose/pack 数量、refs、packed-refs 是否存在）。
//! 环境缺失（`git`/`sha1sum` 不存在）时**硬失败**，没有任何静默 return。
//!
//! 用一个 `#[ignore]` 标出「已确认与真实 git 分歧」的已知限制（见文件末尾），
//! 让它随修复可被点亮，而不是被伪装成通过。

#[path = "interop/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use common::{git_command, mg_command, run, trim, Scratch};

// ---------------------------------------------------------------- 通用工具

fn git_bytes(s: &Scratch, args: &[&str]) -> Vec<u8> {
    let out = run(git_command(s.path(), args));
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn run_with_input(mut cmd: Command, input: &[u8]) -> Output {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("failed to spawn process");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .expect("write stdin");
    child.wait_with_output().expect("wait")
}

/// 用真实 git 建一个「纯 loose 对象」的仓库（两个提交 + 一个 annotated tag + 一个轻量 tag）。
fn fixture_loose_only(s: &Scratch) {
    s.init_with_git();
    s.write("a.txt", "one\n");
    s.write("dir/b.txt", "two\n");
    s.git_ok(&["add", "-A"]);
    s.git_ok(&["commit", "-qm", "c1"]);
    s.write("c.txt", "three\n");
    s.git_ok(&["add", "-A"]);
    s.git_ok(&["commit", "-qm", "c2"]);
    s.git_ok(&["tag", "-a", "va", "-m", "annotated"]);
    s.git_ok(&["tag", "lightweight"]);
}

/// 用真实 git 建一个「loose + pack 混合」的仓库：先全部打包，再产生新的 loose 对象。
fn fixture_mixed(s: &Scratch) {
    fixture_loose_only(s);
    s.git_ok(&["repack", "-adq"]);
    s.write("d.txt", "four\n");
    s.git_ok(&["add", "-A"]);
    s.git_ok(&["commit", "-qm", "c3"]);
}

fn count_objects(s: &Scratch) -> BTreeMap<String, u64> {
    let text = s.git_ok(&["count-objects", "-v"]);
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if let Ok(n) = v.trim().parse() {
                map.insert(k.trim().to_string(), n);
            }
        }
    }
    map
}

/// `git cat-file --batch-all-objects --batch-check`，行排序后作为一个多行字符串。
/// gc 前后必须完全一致（对象集合、类型、大小都不许变）。
fn object_catalog(s: &Scratch) -> String {
    let text = s.git_ok(&["cat-file", "--batch-all-objects", "--batch-check"]);
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort_unstable();
    lines.join("\n")
}

fn object_path(s: &Scratch, oid: &str) -> PathBuf {
    s.join(&format!(".git/objects/{}/{}", &oid[..2], &oid[2..]))
}

/// 递归收集 `.git/objects` 下的 2 位 hex 分片目录里的文件（只看 loose，跳过 pack/info）。
fn loose_files(s: &Scratch) -> Vec<PathBuf> {
    let root = s.join(".git/objects");
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else {
        return out;
    };
    for shard in entries.flatten() {
        if !shard.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = shard.file_name();
        let name = name.to_string_lossy();
        if name.len() != 2 || !name.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        if let Ok(files) = fs::read_dir(shard.path()) {
            for file in files.flatten() {
                if file.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    out.push(file.path());
                }
            }
        }
    }
    out.sort();
    out
}

fn pack_files(s: &Scratch, ext: &str) -> Vec<PathBuf> {
    let dir = s.join(".git/objects/pack");
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn make_writable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(path).expect("stat").permissions();
        perm.set_mode(0o644);
        fs::set_permissions(path, perm).expect("chmod");
    }
}

/// `sha1sum`：环境缺失 → 硬失败（绝不静默跳过）。
fn sha1sum(data: &[u8]) -> String {
    let mut child = Command::new("sha1sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("sha1sum is required for this verification (not found on PATH)");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(data)
        .expect("write to sha1sum");
    let out = child.wait_with_output().expect("sha1sum");
    assert!(out.status.success(), "sha1sum exited non-zero");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .expect("sha1sum output")
        .to_string()
}

/// 损坏夹具的核心断言：真实 git 认为坏，mg 也必须认为坏。
///
/// 两个方向都要求，防止「mg 只是拒绝任何东西」或「夹具其实没坏」这两种假象。
fn assert_both_report_corruption(s: &Scratch, label: &str) {
    let (mg_ok, _, mg_err) = s.mg_raw(&["fsck"]);
    assert!(
        !mg_ok,
        "{label}: mg fsck must exit non-zero on a corrupted repository"
    );
    assert!(
        !mg_err.trim().is_empty(),
        "{label}: mg fsck exited non-zero but printed nothing to stderr"
    );

    let (git_ok, git_out, git_err) = s.git_raw(&["fsck", "--no-progress"]);
    assert!(
        !git_ok,
        "{label}: real git fsck did NOT consider the fixture corrupt — the fixture is wrong. \
         git stdout/stderr:\n{git_out}\n{git_err}"
    );
}

// ---------------------------------------------------------------- mg gc 正向

#[test]
fn gc_products_are_accepted_by_real_git() {
    let s = Scratch::new();
    fixture_loose_only(&s);

    let before = count_objects(&s);
    assert_eq!(
        before.get("packs").copied().unwrap_or(0),
        0,
        "fixture must be loose-only"
    );
    assert_eq!(
        before.get("in-pack").copied().unwrap_or(0),
        0,
        "fixture must be loose-only"
    );
    let loose_before = loose_files(&s).len();
    assert!(loose_before > 0, "fixture must have loose objects");
    let log_before = s.git_ok(&["log", "--all", "--oneline"]);
    let refs_before = s.git_ok(&["for-each-ref"]);
    let catalog_before = object_catalog(&s);
    let (fix_ok, _, fix_err) = s.git_raw(&["fsck", "--no-progress"]);
    assert!(fix_ok, "fixture must already be fsck-clean: {fix_err}");

    let (ok, stdout, stderr) = s.mg_raw(&["gc"]);
    assert!(ok, "mg gc failed (stdout={stdout:?}, stderr={stderr:?})");

    // (a) mg 写的 pack 必须被真实 git 接受（硬证据）。
    let idxs = pack_files(&s, "idx");
    assert_eq!(
        idxs.len(),
        1,
        "expected exactly one mg pack, found {idxs:?}"
    );
    let idx = idxs[0].to_string_lossy().into_owned();
    let (vp_ok, vp_out, vp_err) = s.git_raw(&["verify-pack", "-v", &idx]);
    assert!(
        vp_ok,
        "git verify-pack rejected mg's pack\n--- stdout ---\n{vp_out}\n--- stderr ---\n{vp_err}"
    );

    // (b) 真实 git fsck 无 error。
    let (fsck_ok, fsck_out, fsck_err) = s.git_raw(&["fsck", "--no-progress"]);
    assert!(
        fsck_ok,
        "git fsck after mg gc reported errors\n--- stdout ---\n{fsck_out}\n--- stderr ---\n{fsck_err}"
    );

    // (c) 历史/引用/对象集合逐项不变。
    assert_eq!(
        s.git_ok(&["log", "--all", "--oneline"]),
        log_before,
        "git log --all changed after mg gc"
    );
    assert_eq!(
        s.git_ok(&["for-each-ref"]),
        refs_before,
        "refs changed after mg gc"
    );
    assert_eq!(
        object_catalog(&s),
        catalog_before,
        "object set/type/size changed after mg gc"
    );

    // (d) 每个对象都还能被真实 git 读出。
    for line in catalog_before.lines() {
        let oid = line.split_whitespace().next().expect("oid column");
        s.git_ok(&["cat-file", "-p", oid]);
    }

    // (e) loose 对象数量**确实下降**，并给出前后数字。
    let loose_after = loose_files(&s).len();
    assert!(
        loose_after < loose_before,
        "loose object count did not drop: {loose_before} -> {loose_after}"
    );
    assert_eq!(
        loose_after, 0,
        "mg gc left {loose_after} loose object(s) behind"
    );

    // (f) pack 文件名 = pack 内容（去掉尾部 20 字节）的 sha1 —— 用 sha1sum 独立重算。
    let packs = pack_files(&s, "pack");
    assert_eq!(
        packs.len(),
        1,
        "expected exactly one mg pack file, found {packs:?}"
    );
    let bytes = fs::read(&packs[0]).expect("read pack");
    assert!(bytes.len() > 20, "pack is too small: {} bytes", bytes.len());
    let digest = sha1sum(&bytes[..bytes.len() - 20]);
    let stem = packs[0]
        .file_stem()
        .expect("pack stem")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        stem,
        format!("pack-{digest}"),
        "pack filename must be pack-<sha1 of pack content>"
    );

    // (g) packed-refs 被写出且真实 git 能解析（for-each-ref 已在上面对拍）。
    assert!(
        s.join(".git/packed-refs").is_file(),
        "mg gc must write .git/packed-refs"
    );
    let (head_ok, head_out, head_err) = s.git_raw(&["rev-parse", "HEAD"]);
    assert!(head_ok, "git rev-parse HEAD after gc: {head_err}");
    assert!(
        head_out.trim().len() >= 40,
        "HEAD is not a valid oid after gc"
    );
}

#[test]
fn gc_is_idempotent() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    assert!(s.mg_raw(&["gc"]).0, "first mg gc must succeed");

    let packs_1: Vec<String> = pack_files(&s, "pack")
        .iter()
        .chain(pack_files(&s, "idx").iter())
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let packed_refs_1 = fs::read(s.join(".git/packed-refs")).expect("packed-refs after gc#1");
    let log_1 = s.git_ok(&["log", "--all", "--oneline"]);
    let catalog_1 = object_catalog(&s);
    let (clean_after_first, first_out, first_err) = s.git_raw(&["fsck", "--no-progress"]);
    assert!(
        clean_after_first,
        "git fsck must be clean after gc#1: {first_out}\n{first_err}"
    );

    let (ok, stdout, stderr) = s.mg_raw(&["gc"]);
    assert!(
        ok,
        "second mg gc failed (stdout={stdout:?}, stderr={stderr:?})"
    );

    let packs_2: Vec<String> = pack_files(&s, "pack")
        .iter()
        .chain(pack_files(&s, "idx").iter())
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        packs_2, packs_1,
        "second mg gc changed the pack set (not idempotent): {packs_1:?} -> {packs_2:?}"
    );
    let packed_refs_2 = fs::read(s.join(".git/packed-refs")).expect("packed-refs after gc#2");
    assert_eq!(
        packed_refs_2, packed_refs_1,
        "second mg gc rewrote packed-refs with different bytes"
    );
    assert_eq!(
        s.git_ok(&["log", "--all", "--oneline"]),
        log_1,
        "git log --all changed after the second mg gc"
    );
    assert_eq!(
        object_catalog(&s),
        catalog_1,
        "object set changed after the second mg gc"
    );
    let (fsck_ok, fsck_out, fsck_err) = s.git_raw(&["fsck", "--no-progress"]);
    assert!(
        fsck_ok,
        "git fsck after second mg gc\n--- stdout ---\n{fsck_out}\n--- stderr ---\n{fsck_err}"
    );
}

#[test]
fn gc_on_a_repo_that_already_has_a_git_pack_keeps_everything_valid() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    s.git_ok(&["repack", "-adq"]);
    assert_eq!(
        pack_files(&s, "pack").len(),
        1,
        "fixture must have exactly one git pack"
    );

    s.write("e.txt", "five\n");
    s.git_ok(&["add", "-A"]);
    s.git_ok(&["commit", "-qm", "c3"]);
    assert!(
        !loose_files(&s).is_empty(),
        "fixture must have fresh loose objects"
    );

    let log_before = s.git_ok(&["log", "--all", "--oneline"]);
    let catalog_before = object_catalog(&s);
    let (ok, out, err) = s.mg_raw(&["gc"]);
    assert!(
        ok,
        "mg gc on a repo with an existing git pack failed: {out}\n{err}"
    );

    for idx in pack_files(&s, "idx") {
        let (vp_ok, vp_out, vp_err) = s.git_raw(&["verify-pack", "-v", &idx.to_string_lossy()]);
        assert!(
            vp_ok,
            "git verify-pack rejected {}\n{vp_out}\n{vp_err}",
            idx.display()
        );
    }
    assert_eq!(
        s.git_ok(&["log", "--all", "--oneline"]),
        log_before,
        "log changed"
    );
    assert_eq!(object_catalog(&s), catalog_before, "object set changed");
    assert_eq!(
        loose_files(&s).len(),
        0,
        "the fresh loose objects must be packed"
    );
    let (fsck_ok, fsck_out, fsck_err) = s.git_raw(&["fsck", "--no-progress"]);
    assert!(
        fsck_ok,
        "git fsck after mg gc on a mixed repo\n--- stdout ---\n{fsck_out}\n--- stderr ---\n{fsck_err}"
    );
}

#[test]
fn gc_refuses_to_pack_a_corrupt_loose_object() {
    let s = Scratch::new();
    fixture_loose_only(&s);

    // 造一个没人引用的 loose blob，然后把它的字节改坏。
    let dangling = {
        let out = run_with_input(
            git_command(s.path(), &["hash-object", "-w", "--stdin"]),
            b"unreferenced\n",
        );
        assert!(out.status.success(), "git hash-object -w failed");
        trim(&out.stdout)
    };
    let path = object_path(&s, &dangling);
    assert!(
        path.is_file(),
        "expected a loose object at {}",
        path.display()
    );
    make_writable(&path);
    let mut bytes = fs::read(&path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    let loose_before = loose_files(&s).len();
    let packs_before = pack_files(&s, "pack").len() + pack_files(&s, "idx").len();

    let (ok, stdout, stderr) = s.mg_raw(&["gc"]);
    assert!(
        !ok,
        "mg gc must refuse to pack a corrupt loose object (stdout={stdout:?})"
    );
    assert!(
        !stderr.trim().is_empty(),
        "mg gc failed without a diagnostic"
    );

    assert_eq!(
        loose_files(&s).len(),
        loose_before,
        "mg gc must not delete loose objects when it refuses"
    );
    assert_eq!(
        pack_files(&s, "pack").len() + pack_files(&s, "idx").len(),
        packs_before,
        "mg gc must not leave a half-written pack behind"
    );
}

// ---------------------------------------------------------------- mg fsck 正向

#[test]
fn fsck_accepts_clean_mixed_repo_and_dangling() {
    let s = Scratch::new();
    fixture_mixed(&s);
    // dangling blob（真实 git 只列 dangling，不改退出码）。
    let out = run_with_input(
        git_command(s.path(), &["hash-object", "-w", "--stdin"]),
        b"dangling\n",
    );
    assert!(out.status.success());

    let state = count_objects(&s);
    assert!(
        state.get("in-pack").copied().unwrap_or(0) > 0,
        "fixture must contain a pack"
    );
    assert!(
        state.get("count").copied().unwrap_or(0) > 0,
        "fixture must contain loose objects"
    );

    let (mg_ok, mg_out, mg_err) = s.mg_raw(&["fsck"]);
    assert!(
        mg_ok,
        "mg fsck must accept a clean mixed repo with a dangling blob\nstdout={mg_out}\nstderr={mg_err}"
    );
    assert!(
        mg_out.trim().is_empty(),
        "mg fsck should print nothing on success: {mg_out:?}"
    );
    assert!(
        mg_err.trim().is_empty(),
        "mg fsck should print nothing on success: {mg_err:?}"
    );

    let (full_ok, _, full_err) = s.mg_raw(&["fsck", "--full"]);
    assert!(full_ok, "mg fsck --full must also accept it: {full_err}");

    let (git_ok, git_out, git_err) = s.git_raw(&["fsck", "--no-progress"]);
    assert!(
        git_ok,
        "real git fsck must accept the same fixture: {git_out}\n{git_err}"
    );
}

#[test]
fn fsck_accepts_a_real_git_delta_pack() {
    let s = Scratch::new();
    s.init_with_git();
    let base = "line\n".repeat(200);
    for i in 0..20 {
        s.write("f.txt", &format!("{base}mod {i}\n{}", "tail\n".repeat(50)));
        s.git_ok(&["add", "-A"]);
        s.git_ok(&["commit", "-qm", &format!("c{i}")]);
    }
    s.git_ok(&["repack", "-adfq"]);

    let idx = pack_files(&s, "idx").remove(0);
    let verbose = s.git_ok(&["verify-pack", "-v", &idx.to_string_lossy()]);
    let deltas = verbose
        .lines()
        .filter(|line| {
            let mut parts = line.split_whitespace();
            let first = parts.next().unwrap_or("");
            first.len() == 40
                && first.chars().all(|c| c.is_ascii_hexdigit())
                && line.split_whitespace().count() >= 7
        })
        .count();
    assert!(
        deltas > 0,
        "fixture must contain delta objects, got:\n{verbose}"
    );

    let (mg_ok, mg_out, mg_err) = s.mg_raw(&["fsck"]);
    assert!(
        mg_ok,
        "mg fsck must accept a real git delta pack ({deltas} deltas)\n{mg_out}\n{mg_err}"
    );
    assert!(
        s.git_raw(&["fsck", "--no-progress"]).0,
        "real git must accept it too"
    );
}

// ---------------------------------------------------------------- 人工损坏矩阵

#[test]
fn fsck_detects_loose_object_byte_change() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    let blob = s.git_ok(&["rev-parse", "HEAD:a.txt"]);
    let path = object_path(&s, &blob);
    assert!(path.is_file(), "expected loose blob at {}", path.display());
    make_writable(&path);
    let mut bytes = fs::read(&path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    assert_both_report_corruption(&s, "loose-object byte change");
}

#[test]
fn fsck_detects_truncated_loose_object() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    let blob = s.git_ok(&["rev-parse", "HEAD:a.txt"]);
    let path = object_path(&s, &blob);
    make_writable(&path);
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    assert_both_report_corruption(&s, "truncated loose object");
}

#[test]
fn fsck_detects_truncated_pack() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    s.git_ok(&["repack", "-adq"]);
    let packs = pack_files(&s, "pack");
    assert_eq!(
        packs.len(),
        1,
        "expected exactly one git pack, found {packs:?}"
    );
    // 真实 git 先确认该夹具干净。
    assert!(
        s.git_raw(&["fsck", "--no-progress"]).0,
        "fixture must be git-fsck-clean"
    );
    make_writable(&packs[0]);
    let bytes = fs::read(&packs[0]).unwrap();
    assert!(bytes.len() > 120, "pack too small to truncate safely");
    fs::write(&packs[0], &bytes[..bytes.len() - 100]).unwrap();
    assert_both_report_corruption(&s, "truncated pack");
}

#[test]
fn fsck_detects_corrupt_pack_index() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    s.git_ok(&["repack", "-adq"]);
    let idxs = pack_files(&s, "idx");
    assert_eq!(
        idxs.len(),
        1,
        "expected exactly one git index, found {idxs:?}"
    );
    assert!(
        s.git_raw(&["fsck", "--no-progress"]).0,
        "fixture must be git-fsck-clean"
    );
    make_writable(&idxs[0]);
    let mut bytes = fs::read(&idxs[0]).unwrap();
    // fanout 表从 offset 8 开始（magic 4 + version 4）；改坏 fanout[1]。
    let off = 8 + 4;
    bytes[off] ^= 0xff;
    fs::write(&idxs[0], &bytes).unwrap();
    assert_both_report_corruption(&s, "corrupt pack index (fanout)");
}

#[test]
fn fsck_detects_tree_entry_with_missing_oid() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    let tree = s.git_ok(&["rev-parse", "HEAD^{tree}"]);
    let raw = git_bytes(&s, &["cat-file", "tree", &tree]);
    let nul = raw
        .iter()
        .position(|b| *b == 0)
        .expect("tree entry name terminator");
    let mut patched = raw.clone();
    patched[nul + 1..nul + 21].copy_from_slice(&[0x11; 20]);
    let raw_file = s.join("badtree.raw");
    fs::write(&raw_file, &patched).unwrap();
    let bad_tree = s.git_ok(&[
        "hash-object",
        "-t",
        "tree",
        "-w",
        &raw_file.to_string_lossy(),
    ]);
    let commit = s.git_ok(&["commit-tree", &bad_tree, "-m", "bad tree"]);
    s.git_ok(&["update-ref", "refs/heads/badtree", &commit]);
    assert_both_report_corruption(&s, "tree entry pointing at a missing oid");
}

#[test]
fn fsck_detects_commit_with_missing_parent() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    let tree = s.git_ok(&["rev-parse", "HEAD^{tree}"]);
    let content = format!(
        "tree {tree}\nparent 2222222222222222222222222222222222222222\n\
         author T <t@e> 1700000000 +0000\ncommitter T <t@e> 1700000000 +0000\n\nbad parent\n"
    );
    let file = s.join("badcommit.raw");
    fs::write(&file, content).unwrap();
    let commit = s.git_ok(&["hash-object", "-t", "commit", "-w", &file.to_string_lossy()]);
    s.git_ok(&["update-ref", "refs/heads/badparent", &commit]);
    assert_both_report_corruption(&s, "commit with a missing parent");
}

#[test]
fn fsck_detects_commit_missing_tree_line() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    // `--literally` 允许写出 git 平时会拒绝的畸形 commit（缺 tree 行）。
    let content = "parent 3333333333333333333333333333333333333333\n\
                   author T <t@e> 1700000000 +0000\ncommitter T <t@e> 1700000000 +0000\n\nno tree\n";
    let file = s.join("notree.raw");
    fs::write(&file, content).unwrap();
    let commit = s.git_ok(&[
        "hash-object",
        "--literally",
        "-t",
        "commit",
        "-w",
        &file.to_string_lossy(),
    ]);
    // update-ref 会拒绝指向不可解析对象的引用，所以直接写 ref 文件把它变成可达对象。
    let ref_file = s.join(".git/refs/heads/notree");
    fs::create_dir_all(ref_file.parent().unwrap()).unwrap();
    fs::write(&ref_file, format!("{commit}\n")).unwrap();
    assert_both_report_corruption(&s, "reachable commit missing its tree line");
}

#[test]
fn fsck_ignores_a_clean_dangling_object() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    let out = run_with_input(
        git_command(s.path(), &["hash-object", "-w", "--stdin"]),
        b"nobody references me\n",
    );
    assert!(out.status.success());
    let dangling = trim(&out.stdout);

    let (git_ok, git_out, git_err) = s.git_raw(&["fsck", "--no-progress"]);
    assert!(
        git_ok,
        "real git must exit 0 on a clean dangling object: {git_out}\n{git_err}"
    );
    assert!(
        git_out.contains(&dangling) || git_err.contains(&dangling),
        "sanity: git should mention the dangling object {dangling}"
    );

    let (mg_ok, mg_out, mg_err) = s.mg_raw(&["fsck"]);
    assert!(
        mg_ok,
        "mg fsck must exit 0 on a clean dangling object (matching git)\nstdout={mg_out}\nstderr={mg_err}"
    );
    assert!(
        mg_err.trim().is_empty(),
        "mg fsck must not warn about a dangling object: {mg_err:?}"
    );
}

// ---------------------------------------------------------------- 已知分歧

/// **已确认与真实 git 的分歧**（不作为「必须通过项」，用 `#[ignore]` 保留）：
/// 真实 `git fsck` 会扫描对象库里**全部**对象（含不可达的 loose 对象）；
/// `mg fsck` 只从 refs 可达集 + index + pack 出发，因此**漏报**不可达 loose 对象的损坏。
///
/// 手工证据（本文件写就时实测）：
/// ```text
/// git hash-object -w --stdin <<< "unreferenced"   # 造一个 dangling blob
/// # 改坏它的字节后：
/// git fsck --no-progress  -> exit 1（error: inflate: data stream error ...）
/// mg  fsck                -> exit 0（无输出）
/// ```
/// 任务书 §3.1 把 fsck 的范围限定为「从全部 ref 出发」，所以这更像**范围外/已知限制**
/// 而不是「实现错误」；但它确实与「判定方向必须与真实 git 一致」存在张力，
/// 故单独登记，待 controller 裁决。
#[test]
#[ignore = "known divergence: mg fsck only walks reachable objects; git fsck also scans unreachable loose objects"]
fn fsck_detects_corruption_in_unreachable_objects() {
    let s = Scratch::new();
    fixture_loose_only(&s);
    let out = run_with_input(
        git_command(s.path(), &["hash-object", "-w", "--stdin"]),
        b"unreferenced\n",
    );
    assert!(out.status.success());
    let dangling = trim(&out.stdout);
    let path = object_path(&s, &dangling);
    make_writable(&path);
    let mut bytes = fs::read(&path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    fs::write(&path, &bytes).unwrap();
    assert_both_report_corruption(&s, "corrupt unreachable loose object");
}

// ---------------------------------------------------------------- e2e 收口

#[test]
fn e2e_local_pipeline_matches_git() {
    let s = Scratch::new();
    s.init_with_mg();

    // hash-object 与真实 git 逐字节一致。
    let (mg_ok, mg_hash, mg_err) = {
        let out = run_with_input(
            mg_command(s.path(), &["hash-object", "--stdin"]),
            b"hello\n",
        );
        (out.status.success(), trim(&out.stdout), trim(&out.stderr))
    };
    assert!(mg_ok, "mg hash-object failed: {mg_err}");
    let (git_ok, git_hash, git_err) = {
        let out = run_with_input(
            git_command(s.path(), &["hash-object", "--stdin"]),
            b"hello\n",
        );
        (out.status.success(), trim(&out.stdout), trim(&out.stderr))
    };
    assert!(git_ok, "git hash-object failed: {git_err}");
    assert_eq!(mg_hash, git_hash, "mg hash-object != git hash-object");

    s.write("hello.txt", "hello\n");
    s.mg_ok(&["add", "."]);
    // status 对拍：真实 git 读同一个 .git。
    assert_same_output(&s, "status --porcelain", &["status", "--porcelain"]);
    // index 对拍：git 看到的 stage 条目必须指向正确 blob。
    let ls = s.git_ok(&["ls-files", "--stage"]);
    assert_eq!(
        ls,
        format!("100644 {git_hash} 0\thello.txt"),
        "git ls-files --stage after mg add is wrong"
    );

    s.mg_ok(&["commit", "-m", "first"]);
    assert_same_output(&s, "log --oneline", &["log", "--oneline"]);
    assert_eq!(
        s.git_ok(&["status", "--porcelain"]),
        "",
        "worktree should be clean after commit"
    );

    // tag（annotated + lightweight）与 branch/switch。
    s.mg_ok(&["tag", "v1"]);
    s.mg_ok(&["tag", "-a", "va", "-m", "annotated"]);
    assert_eq!(s.git_ok(&["tag", "-l"]), "v1\nva", "tags after mg tag");
    assert_eq!(
        s.git_ok(&["cat-file", "-t", "va"]),
        "tag",
        "mg tag -a must write a real tag object"
    );

    s.mg_ok(&["branch", "side"]);
    assert_eq!(
        s.git_ok(&["rev-parse", "side"]),
        s.git_ok(&["rev-parse", "HEAD"])
    );
    s.mg_ok(&["switch", "side"]);
    assert_eq!(s.git_ok(&["symbolic-ref", "HEAD"]), "refs/heads/side");

    // 在 side 上提交，再回 main 造出可对拍的 diff。
    s.write("hello.txt", "hello\nside\n");
    s.mg_ok(&["add", "hello.txt"]);
    s.mg_ok(&["commit", "-m", "side change"]);
    s.mg_ok(&["switch", "main"]);
    s.write("hello.txt", "hello\nmain\n");
    assert_same_output(&s, "diff", &["diff"]);
    s.mg_ok(&["add", "hello.txt"]);
    s.mg_ok(&["commit", "-m", "main change"]);

    // merge（非快进、会冲突的完整历史）—— 平铺路径，避开已知的 D1（嵌套路径快进合并）。
    let (merge_ok, _, merge_err) = s.mg_raw(&["merge", "side"]);
    assert!(!merge_ok, "merge should stop with a conflict");
    assert!(
        merge_err.contains("conflict") || merge_err.contains("merge"),
        "unexpected merge error: {merge_err}"
    );
    assert_eq!(
        s.git_ok(&["status", "--porcelain"]),
        "UU hello.txt",
        "git must see an unmerged path after mg merge conflict"
    );
    let stage = s.git_ok(&["ls-files", "--stage"]);
    assert_eq!(
        stage.lines().count(),
        3,
        "conflict must produce 3 stage entries, got:\n{stage}"
    );

    // 解决冲突并提交（2 个 parent）。
    s.write("hello.txt", "hello\nresolved\n");
    s.mg_ok(&["add", "hello.txt"]);
    s.mg_ok(&["commit", "-m", "merged"]);
    let parents = s.git_ok(&["cat-file", "-p", "HEAD"]);
    assert_eq!(
        parents.lines().filter(|l| l.starts_with("parent ")).count(),
        2,
        "merge commit must have 2 parents"
    );
    assert!(
        s.git_raw(&["fsck", "--no-progress"]).0,
        "git fsck after merge"
    );

    // reset --hard 与 checkout。
    s.write("hello.txt", "dirty\n");
    s.mg_ok(&["reset", "--hard", "HEAD"]);
    assert_eq!(
        s.git_ok(&["status", "--porcelain"]),
        "",
        "reset --hard must clean the worktree"
    );
    s.mg_ok(&["checkout", "side"]);
    assert_eq!(s.git_ok(&["symbolic-ref", "HEAD"]), "refs/heads/side");
    assert_eq!(
        s.read("hello.txt"),
        "hello\nside\n",
        "checkout must restore tree contents"
    );

    // fsck + gc 收口，并交给真实 git 判定。
    s.mg_ok(&["fsck"]);
    s.mg_ok(&["gc"]);
    let (git_ok, git_out, git_err) = s.git_raw(&["fsck", "--no-progress"]);
    assert!(
        git_ok,
        "git fsck after the e2e pipeline + mg gc\n--- stdout ---\n{git_out}\n--- stderr ---\n{git_err}"
    );
}

#[test]
fn e2e_clone_fetch_push_fsck_gc_close_out() {
    let s = Scratch::new();
    // origin：真实 git 仓库，含一个 tag（避开已知 D2：无 tag 远端 clone 失败）。
    s.write("origin/a.txt", "a\n");
    let origin = s.join("origin");
    let go = |args: &[&str]| git_command(&origin, args);
    assert!(run(go(&["init", "--quiet", "."])).status.success());
    assert!(run(go(&["add", "."])).status.success());
    assert!(run(go(&["commit", "-qm", "one"])).status.success());
    assert!(run(go(&["tag", "v0"])).status.success());

    // mg clone file://（真实 git 复查 clone 结果）。
    let url = format!("file://{}", origin.display());
    s.mg_ok(&["clone", &url, "copy"]);
    let copy = s.join("copy");
    let gc1 = |args: &[&str]| git_command(&copy, args);
    assert!(
        run(gc1(&["fsck", "--no-progress"])).status.success(),
        "git fsck in the mg clone must pass"
    );
    assert_eq!(
        trim(&run(gc1(&["log", "--oneline"])).stdout),
        trim(&run(go(&["log", "--oneline"])).stdout),
        "clone log must equal origin log"
    );

    // bare 远端：真实 git 建、真实 git 复核。
    let bare = s.join("bare.git");
    fs::create_dir_all(&bare).unwrap();
    let gb = |args: &[&str]| git_command(&bare, args);
    assert!(run(gb(&["init", "--quiet", "--bare", "."]))
        .status
        .success());
    let bare_url = format!("file://{}", bare.display());
    assert!(run(go(&["push", "-q", &bare_url, "main"])).status.success());
    assert!(run(go(&["push", "-q", &bare_url, "v0"])).status.success());

    let copy2 = s.join("copy2");
    s.mg_ok(&["clone", &bare_url, "copy2"]);
    let mc2 = |args: &[&str]| mg_command(&copy2, args);
    let gc2 = |args: &[&str]| git_command(&copy2, args);
    std::fs::write(copy2.join("b.txt"), "b\n").unwrap();
    assert!(run(mc2(&["add", "b.txt"])).status.success());
    assert!(run(mc2(&["commit", "-m", "two"])).status.success());

    // mg push 到 bare 远端（当前 T13/T14 的 push 语义下应成功）。
    let push = run(mc2(&["push", "origin", "main"]));
    assert!(
        push.status.success(),
        "mg push to a bare remote failed: {}",
        String::from_utf8_lossy(&push.stderr)
    );
    assert_eq!(
        trim(&run(gb(&["rev-parse", "main"])).stdout),
        trim(&run(gc2(&["rev-parse", "HEAD"])).stdout),
        "bare remote main must equal the pushed HEAD"
    );

    // fetch：远端（bare.git）前进后，mg fetch 必须更新 remote-tracking ref。
    let adv = s.join("adv");
    assert!(run(git_command(
        s.path(),
        &["clone", "-q", &bare_url, &adv.to_string_lossy()]
    ))
    .status
    .success());
    let ga = |args: &[&str]| git_command(&adv, args);
    assert!(run(ga(&["commit", "-qm", "three", "--allow-empty"]))
        .status
        .success());
    assert!(run(ga(&["push", "-q", "origin", "main"])).status.success());
    let new_remote = trim(&run(gb(&["rev-parse", "main"])).stdout);
    let fetch = run(mc2(&["fetch", "origin"]));
    assert!(
        fetch.status.success(),
        "mg fetch failed: {}",
        String::from_utf8_lossy(&fetch.stderr)
    );
    assert_eq!(
        trim(&run(gc2(&["rev-parse", "refs/remotes/origin/main"])).stdout),
        new_remote,
        "fetch must update refs/remotes/origin/main"
    );

    // fsck / gc 收口，交给真实 git 判定。
    assert!(run(mc2(&["fsck"])).status.success(), "mg fsck in the clone");
    assert!(run(mc2(&["gc"])).status.success(), "mg gc in the clone");
    let after = run(gc2(&["fsck", "--no-progress"]));
    assert!(
        after.status.success(),
        "git fsck after clone/fetch/push + mg gc: {}",
        String::from_utf8_lossy(&after.stderr)
    );
}

fn assert_same_output(s: &Scratch, what: &str, args: &[&str]) {
    let mg = trim(&s.mg(args).stdout);
    let git = trim(&s.git(args).stdout);
    assert_eq!(
        mg, git,
        "{what}: mg output differs from real git\n--- mg ---\n{mg}\n--- git ---\n{git}"
    );
}
