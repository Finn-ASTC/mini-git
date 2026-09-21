//! T15 收口：`mg fsck` / `mg gc` 的一致性用例 + 整条 CLI 的端到端对拍。
//!
//! **文件位置说明（与任务书的差异，已在上报里说明）**：任务写的是
//! `tests/interop/fsck_gc.rs`，但 `tests/interop/` 是 `tests/interop/main.rs`
//! （CONTROLLER-OWNED）的**子模块目录**：放在那里的文件只有在 `main.rs` 里写了
//! `mod fsck_gc;` 才会被编译。直接放进去 = 一个永不编译、永不执行的文件（正是任务书
//! 反复警告的「假绿」）。cargo 只把 `tests/` 的**直接子文件**当成独立 test target，
//! 所以本文件放在 `tests/fsck_gc.rs`，并用 `#[path]` 复用 verifier-only 的
//! `tests/interop/common/mod.rs`（一行都没有改 controller 的文件）。
//!
//! 环境缺失的处理（任务书 §「测试不得 skip 即通过」）：本文件里所有依赖外部工具的
//! 检查都**硬失败**（`expect`/`assert`），没有任何「拿不到就 return 成通过」的分支。
//! 唯一显式豁免的是 `#[ignore]` 标注的两处，理由逐条写在属性里。

#[path = "interop/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{assert_same, Scratch};

// ------------------------------------------------------------------ 小工具

/// 真实 git，取原始 stdout 字节（内容对拍要用二进制）。
fn git_bytes(scratch: &Scratch, args: &[&str]) -> Vec<u8> {
    let out = common::run(common::git_command(scratch.path(), args));
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// `git count-objects -v` 的键值对。
fn count_objects(scratch: &Scratch) -> BTreeMap<String, u64> {
    let text = scratch.git_ok(&["count-objects", "-v"]);
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if let Ok(value) = value.trim().parse() {
                map.insert(key.trim().to_string(), value);
            }
        }
    }
    map
}

/// 对象库的**内容快照**：`oid → (type, 原始内容)`，按 oid 排序。
/// gc 前后必须逐字节相同（数量、类型、内容都不能变）。
fn object_snapshot(scratch: &Scratch) -> Vec<(String, String, Vec<u8>)> {
    let listing = scratch.git_ok(&["cat-file", "--batch-all-objects", "--batch-check"]);
    let mut entries: Vec<(String, String)> = listing
        .lines()
        .map(|line| {
            let mut parts = line.split_whitespace();
            let oid = parts.next().expect("oid column").to_string();
            let kind = parts.next().expect("type column").to_string();
            (oid, kind)
        })
        .collect();
    entries.sort();
    entries
        .into_iter()
        .map(|(oid, kind)| {
            let content = git_bytes(scratch, &["cat-file", &kind, &oid]);
            (oid, kind, content)
        })
        .collect()
}

/// coreutils 的 `sha1sum`（环境缺失 → 硬失败，绝不静默跳过）。
fn sha1_of(bytes: &[u8]) -> String {
    let mut child = Command::new("sha1sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("this interop check needs sha1sum (coreutils) on PATH");
    child
        .stdin
        .as_mut()
        .expect("sha1sum stdin")
        .write_all(bytes)
        .expect("write to sha1sum");
    let out = child.wait_with_output().expect("sha1sum output");
    assert!(out.status.success(), "sha1sum failed");
    String::from_utf8(out.stdout)
        .expect("sha1sum is ASCII")
        .split_whitespace()
        .next()
        .expect("sha1sum prints a digest")
        .to_string()
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    (0..hex.len() / 2)
        .map(|idx| u8::from_str_radix(&hex[idx * 2..idx * 2 + 2], 16).expect("hex digit"))
        .collect()
}

fn pack_files(scratch: &Scratch) -> Vec<PathBuf> {
    let dir = scratch.join(".git/objects/pack");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("pack"))
        .collect();
    found.sort();
    found
}

fn loose_object_files(scratch: &Scratch) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let objects = scratch.join(".git/objects");
    for shard in fs::read_dir(&objects).expect("objects dir") {
        let shard = shard.expect("shard entry").path();
        if !shard.is_dir() || shard.file_name().and_then(|n| n.to_str()) == Some("pack") {
            continue;
        }
        for entry in fs::read_dir(&shard).expect("object entry") {
            let path = entry.expect("object entry").path();
            if path.is_file() {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// 让 0444 的 git 文件可写（损坏场景要改字节）。
fn make_writable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).expect("chmod");
}

/// 真实 git 的 fsck：exit 0，且**没有** `error:` 行（dangling 不算错误）。
fn assert_git_fsck_clean(scratch: &Scratch, what: &str) {
    let out = common::run(common::git_command(
        scratch.path(),
        &["fsck", "--no-progress"],
    ));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "{what}: git fsck exit {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        out.status.code()
    );
    for line in stdout.lines().chain(stderr.lines()) {
        assert!(
            !line.starts_with("error:"),
            "{what}: git fsck reported an error: {line}"
        );
    }
}

/// `mg fsck` 干净：exit 0，stdout/stderr 都为空（与 git 一样「没问题就不说话」）。
fn assert_mg_fsck_clean(scratch: &Scratch, args: &[&str], what: &str) {
    let out = scratch.mg(args);
    assert!(
        out.status.success(),
        "{what}: mg {} exit {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        args.join(" "),
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "{what}: a clean repository must not print to stdout, got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.stderr.is_empty(),
        "{what}: a clean repository must not print to stderr, got {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 在任意目录里跑 mg / git（`clone` 的 dst 不是 `Scratch` 时用）。
fn run_ok(dir: &Path, mg: bool, args: &[&str]) -> String {
    let command = if mg {
        common::mg_command(dir, args)
    } else {
        common::git_command(dir, args)
    };
    let out = common::run(command);
    common::assert_success(
        &format!("{} {}", if mg { "mg" } else { "git" }, args.join(" ")),
        &out,
    );
    common::trim(&out.stdout)
}

/// 同 [`assert_git_fsck_clean`]，但接受任意目录。
fn assert_git_fsck_clean_in(dir: &Path, what: &str) {
    let out = common::run(common::git_command(dir, &["fsck", "--no-progress"]));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "{what}: git fsck exit {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        out.status.code()
    );
    for line in stdout.lines().chain(stderr.lines()) {
        assert!(
            !line.starts_with("error:"),
            "{what}: git fsck reported an error: {line}"
        );
    }
}

/// 一个「真实 git 写的、含嵌套路径/tag/分支、并且已经被 gc 打包」的仓库。
fn real_git_repository_with_a_pack(scratch: &Scratch) {
    scratch.init_with_git();
    scratch.write("a.txt", "alpha\n");
    scratch.write("dir/b.txt", "beta\n");
    scratch.write("dir/deep/c.txt", "gamma\n");
    scratch.write("with space.txt", "spaced\n");
    scratch.git_commit_all("c1");
    scratch.write("a.txt", "alpha v2\n");
    scratch.git_commit_all("c2");
    scratch.git_ok(&["tag", "-a", "v1", "-m", "annotated"]);
    scratch.git_ok(&["tag", "light"]);
    scratch.git_ok(&["branch", "feature"]);
    scratch.git_ok(&["gc", "-q"]);
    assert!(
        !pack_files(scratch).is_empty(),
        "fixture must contain a packfile after `git gc`"
    );
}

/// 一个 mg 自己写的仓库（嵌套路径 + annotated tag + 额外分支）。
fn mg_repository(scratch: &Scratch) {
    scratch.init_with_mg();
    scratch.write("a.txt", "alpha\n");
    scratch.write("dir/b.txt", "beta\n");
    scratch.mg_ok(&["add", "."]);
    scratch.mg_ok(&["commit", "-m", "first"]);
    scratch.write("a.txt", "alpha v2\n");
    scratch.mg_ok(&["add", "a.txt"]);
    scratch.mg_ok(&["commit", "-m", "second"]);
    scratch.mg_ok(&["tag", "v1"]);
    scratch.mg_ok(&["tag", "-a", "va", "-m", "annotated tag"]);
    scratch.mg_ok(&["branch", "side"]);
}

// ------------------------------------------------------- fsck：正向

#[test]
fn mg_fsck_is_clean_on_a_real_git_repository_with_a_pack() {
    let scratch = Scratch::new();
    real_git_repository_with_a_pack(&scratch);

    assert_git_fsck_clean(&scratch, "fixture");
    assert_mg_fsck_clean(&scratch, &["fsck"], "packed git repository");
    // `--full` 与默认等价（git 的 `--full` 也正是默认行为），两者都必须干净。
    assert_mg_fsck_clean(&scratch, &["fsck", "--full"], "packed git repository (--full)");
}

#[test]
fn mg_fsck_ignores_dangling_objects() {
    let scratch = Scratch::new();
    scratch.init_with_git();
    scratch.write("a.txt", "alpha\n");
    scratch.git_commit_all("c1");
    // 与 git 一致：存在但不可达的对象只是 dangling，不是错误。
    let dangling = scratch.git_ok(&["hash-object", "-w", "--stdin"]);
    assert_eq!(
        scratch.git_ok(&["rev-parse", "--verify", &dangling]),
        dangling
    );

    assert_mg_fsck_clean(&scratch, &["fsck"], "repository with a dangling object");
    assert_git_fsck_clean(&scratch, "repository with a dangling object");
}

#[test]
fn mg_fsck_is_clean_on_an_mg_written_repository_and_after_gc() {
    let scratch = Scratch::new();
    mg_repository(&scratch);

    assert_mg_fsck_clean(&scratch, &["fsck"], "fresh mg repository");
    scratch.mg_ok(&["gc"]);
    assert_mg_fsck_clean(&scratch, &["fsck"], "mg repository after mg gc");
    assert_git_fsck_clean(&scratch, "mg repository after mg gc");
}

#[test]
fn mg_fsck_accepts_an_unborn_repository() {
    let scratch = Scratch::new();
    scratch.init_with_mg();
    // HEAD 指向尚未创建的分支：git 只给 notice，不算错误。
    assert_mg_fsck_clean(&scratch, &["fsck"], "unborn HEAD");
    assert_git_fsck_clean(&scratch, "unborn HEAD");
}

// ------------------------------------------------------- fsck：损坏场景

#[test]
fn mg_fsck_reports_a_corrupted_loose_object() {
    let scratch = Scratch::new();
    scratch.init_with_git();
    scratch.write("a.txt", "alpha\n");
    scratch.git_commit_all("c1");

    let victim = loose_object_files(&scratch)
        .into_iter()
        .next()
        .expect("a loose object must exist");
    let oid = format!(
        "{}{}",
        victim.parent().unwrap().file_name().unwrap().to_str().unwrap(),
        victim.file_name().unwrap().to_str().unwrap()
    );
    make_writable(&victim);
    fs::write(&victim, b"this is not a zlib stream at all").expect("corrupt the object");

    let out = scratch.mg(&["fsck"]);
    assert!(
        !out.status.success(),
        "a corrupted loose object must make mg fsck fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&oid),
        "mg fsck must name the corrupted object {oid}, got {stderr:?}"
    );
    assert!(
        stderr.lines().any(|line| line.starts_with("error: ")),
        "mg fsck must print `error: ...` lines to stderr, got {stderr:?}"
    );

    // 同样的仓库，真实 git 也是非 0（结论方向一致）。
    let git_out = common::run(common::git_command(
        scratch.path(),
        &["fsck", "--no-progress"],
    ));
    assert!(
        !git_out.status.success(),
        "real git fsck must also fail on this fixture"
    );
}

#[test]
fn mg_fsck_reports_a_truncated_pack() {
    let scratch = Scratch::new();
    real_git_repository_with_a_pack(&scratch);
    let pack = pack_files(&scratch).remove(0);

    let bytes = fs::read(&pack).expect("read pack");
    make_writable(&pack);
    fs::write(&pack, &bytes[..bytes.len() - 100]).expect("truncate the pack");

    let out = scratch.mg(&["fsck"]);
    assert!(!out.status.success(), "a truncated pack must fail mg fsck");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(pack.file_name().unwrap().to_str().unwrap()),
        "mg fsck must name the truncated pack, got {stderr:?}"
    );
}

#[test]
fn mg_fsck_reports_a_pack_whose_object_count_disagrees_with_its_index() {
    let scratch = Scratch::new();
    real_git_repository_with_a_pack(&scratch);
    let pack = pack_files(&scratch).remove(0);
    let index = pack.with_extension("idx");
    assert!(index.is_file());

    // 改掉 pack 头里的 count，再用 sha1sum 重算尾部校验和 —— 这样打到的就是
    // 「header 的 count ≠ .idx 的对象数」这条检查，而不是校验和检查。
    let mut bytes = fs::read(&pack).expect("read pack");
    let real_count = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    bytes[8..12].copy_from_slice(&(real_count + 7).to_be_bytes());
    let body_len = bytes.len() - 20;
    let digest = hex_to_bytes(&sha1_of(&bytes[..body_len]));
    bytes[body_len..].copy_from_slice(&digest);
    make_writable(&pack);
    fs::write(&pack, &bytes).expect("rewrite the pack");

    let out = scratch.mg(&["fsck"]);
    assert!(
        !out.status.success(),
        "a pack whose header count disagrees with its .idx must fail mg fsck"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("declares"),
        "mg fsck must report the count mismatch, got {stderr:?}"
    );
}

#[test]
fn mg_fsck_reports_an_index_without_its_pack() {
    let scratch = Scratch::new();
    real_git_repository_with_a_pack(&scratch);
    let pack = pack_files(&scratch).remove(0);
    make_writable(&pack);
    fs::remove_file(&pack).expect("remove the .pack but keep the .idx");

    let out = scratch.mg(&["fsck"]);
    assert!(
        !out.status.success(),
        "an .idx without its .pack must fail mg fsck"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no matching .pack"),
        "mg fsck must explain the orphan .idx, got {stderr:?}"
    );
}

#[test]
fn mg_fsck_reports_a_pack_without_its_index() {
    let scratch = Scratch::new();
    real_git_repository_with_a_pack(&scratch);
    let index = pack_files(&scratch).remove(0).with_extension("idx");
    make_writable(&index);
    fs::remove_file(&index).expect("remove the .idx but keep the .pack");

    let out = scratch.mg(&["fsck"]);
    assert!(
        !out.status.success(),
        "a .pack without its .idx must fail mg fsck"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(".pack without a matching .idx"),
        "mg fsck must explain the orphan .pack, got {stderr:?}"
    );
}

#[test]
fn mg_fsck_reports_a_broken_index_trailer() {
    let scratch = Scratch::new();
    scratch.init_with_git();
    scratch.write("a.txt", "alpha\n");
    scratch.git_commit_all("c1");
    scratch.write("b.txt", "beta\n");
    scratch.git_ok(&["add", "-A"]);

    let index = scratch.join(".git/index");
    let mut bytes = fs::read(&index).expect("read index");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    make_writable(&index);
    fs::write(&index, &bytes).expect("flip a trailer byte");

    let out = scratch.mg(&["fsck"]);
    assert!(
        !out.status.success(),
        "a corrupted index trailer must fail mg fsck"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("index"),
        "mg fsck must mention the index, got {stderr:?}"
    );
}

#[test]
fn mg_fsck_reports_an_index_entry_whose_object_is_gone() {
    let scratch = Scratch::new();
    scratch.init_with_git();
    scratch.write("a.txt", "alpha\n");
    scratch.git_commit_all("c1");
    scratch.write("b.txt", "beta\n");
    scratch.git_ok(&["add", "-A"]);

    let oid = scratch
        .git_ok(&["ls-files", "--stage", "b.txt"])
        .split_whitespace()
        .nth(1)
        .expect("staged oid")
        .to_string();
    let path = scratch.join(&format!(".git/objects/{}/{}", &oid[..2], &oid[2..]));
    make_writable(&path);
    fs::remove_file(&path).expect("delete the staged blob");

    let out = scratch.mg(&["fsck"]);
    assert!(
        !out.status.success(),
        "an index entry pointing at a missing object must fail mg fsck"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&oid),
        "mg fsck must name the missing object {oid}, got {stderr:?}"
    );
}

// ------------------------------------------------------- gc

#[test]
fn mg_gc_packs_every_loose_object_and_real_git_accepts_the_pack() {
    let scratch = Scratch::new();
    mg_repository(&scratch);

    let before_counts = count_objects(&scratch);
    let before_objects = object_snapshot(&scratch);
    let before_log = scratch.git_ok(&["log", "--all", "--oneline"]);
    let before_refs = scratch.git_ok(&["show-ref"]);
    assert!(before_counts["count"] > 0, "fixture must have loose objects");
    assert_eq!(before_counts["packs"], 0, "fixture must not have a pack yet");

    scratch.mg_ok(&["gc"]);

    // 1. 对象数下降：loose 清零、全部进 pack。
    let after_counts = count_objects(&scratch);
    assert_eq!(after_counts["count"], 0, "no loose object may survive mg gc");
    assert_eq!(after_counts["packs"], 1, "mg gc must produce exactly one pack");
    assert_eq!(
        after_counts["in-pack"], before_counts["count"],
        "every loose object must be in the pack"
    );

    // 2. 对象集合与内容逐字节不变。
    assert_eq!(
        object_snapshot(&scratch),
        before_objects,
        "mg gc must not change any object"
    );

    // 3. 历史与引用不变。
    assert_same(
        "log --all --oneline after gc",
        &scratch.git_ok(&["log", "--all", "--oneline"]),
        &before_log,
    );
    assert_same(
        "show-ref after gc",
        &scratch.git_ok(&["show-ref"]),
        &before_refs,
    );
    scratch.mg_ok(&["status", "--porcelain"]);

    // 4. 文件名就是 pack 内容的 sha1（与 git 的命名规则一致）。
    let pack = pack_files(&scratch).remove(0);
    let bytes = fs::read(&pack).expect("read the pack mg wrote");
    let expected = format!("pack-{}", sha1_of(&bytes[..bytes.len() - 20]));
    assert_eq!(
        pack.file_stem().unwrap().to_str().unwrap(),
        expected,
        "mg must name the pack like git does (sha1 of the pack contents)"
    );
    assert!(
        pack.with_extension("idx").is_file(),
        ".idx must sit next to the .pack"
    );

    // 5. 真实 git 能读、能校验、能重新打包。
    assert_git_fsck_clean(&scratch, "after mg gc");
    let idx = pack.with_extension("idx");
    let verified = scratch.git_ok(&["verify-pack", "-v", idx.to_str().unwrap()]);
    assert!(
        verified.contains(": ok"),
        "git verify-pack must accept the mg pack, got {verified:?}"
    );
    scratch.git_ok(&["repack", "-adf"]);
    assert_git_fsck_clean(&scratch, "after git repack of the mg pack");

    // 6. mg 自己也认为仓库是干净的。
    assert_mg_fsck_clean(&scratch, &["fsck"], "after mg gc");
}

#[test]
fn mg_gc_is_idempotent() {
    let scratch = Scratch::new();
    mg_repository(&scratch);
    scratch.mg_ok(&["gc"]);

    let packs_before = pack_files(&scratch);
    let refs_before = fs::read(scratch.join(".git/packed-refs")).expect("packed-refs");
    let log_before = scratch.git_ok(&["log", "--all", "--oneline"]);

    scratch.mg_ok(&["gc"]);

    assert_eq!(
        pack_files(&scratch),
        packs_before,
        "a second mg gc must not create another pack"
    );
    assert_eq!(
        fs::read(scratch.join(".git/packed-refs")).expect("packed-refs"),
        refs_before,
        "a second mg gc must leave packed-refs byte-identical"
    );
    assert_eq!(scratch.git_ok(&["log", "--all", "--oneline"]), log_before);
    assert_eq!(count_objects(&scratch)["packs"], 1);
    assert_git_fsck_clean(&scratch, "after two mg gc runs");
    assert_mg_fsck_clean(&scratch, &["fsck"], "after two mg gc runs");
}

#[test]
fn mg_gc_writes_packed_refs_that_real_git_would_write() {
    let scratch = Scratch::new();
    mg_repository(&scratch);
    scratch.mg_ok(&["gc"]);

    let ours = fs::read(scratch.join(".git/packed-refs")).expect("mg packed-refs");
    // 引用仍然全部可解析（loose + packed 合并后与 gc 前一致）。
    assert_eq!(scratch.git_ok(&["rev-parse", "refs/tags/v1"]).len(), 40);
    let peeled = scratch.git_ok(&["rev-parse", "refs/tags/va^{commit}"]);
    assert_eq!(peeled, scratch.git_ok(&["rev-parse", "refs/heads/main"]));

    // 让真实 git 重新生成同一份 packed-refs：两边必须逐字节相同
    // （头部 `# pack-refs with: peeled fully-peeled sorted ` + 排序 + annotated tag 的 `^` 行）。
    scratch.git_ok(&["pack-refs", "--all"]);
    let theirs = fs::read(scratch.join(".git/packed-refs")).expect("git packed-refs");
    assert_eq!(
        String::from_utf8_lossy(&ours),
        String::from_utf8_lossy(&theirs),
        "mg packed-refs must match what `git pack-refs --all` writes"
    );
    assert!(
        String::from_utf8_lossy(&ours).contains("^"),
        "an annotated tag must get its peeled line: {}",
        String::from_utf8_lossy(&ours)
    );
    assert_git_fsck_clean(&scratch, "after git pack-refs rewrote our file");
}

#[test]
fn mg_gc_refuses_to_pack_a_corrupt_loose_object() {
    let scratch = Scratch::new();
    scratch.init_with_mg();
    scratch.write("a.txt", "alpha\n");
    scratch.mg_ok(&["add", "."]);
    scratch.mg_ok(&["commit", "-m", "first"]);

    let loose = loose_object_files(&scratch);
    assert!(!loose.is_empty());
    let victim = loose[0].clone();
    make_writable(&victim);
    fs::write(&victim, b"garbage").expect("corrupt the object");

    let out = scratch.mg(&["gc"]);
    assert!(
        !out.status.success(),
        "mg gc must refuse to pack a corrupt object instead of writing a broken pack"
    );
    assert_eq!(
        loose_object_files(&scratch).len(),
        loose.len(),
        "mg gc must not delete loose objects when it fails"
    );
    assert!(
        pack_files(&scratch).is_empty(),
        "mg gc must not leave a pack behind when it fails"
    );
}

#[test]
fn mg_gc_on_an_empty_repository_is_a_no_op() {
    let scratch = Scratch::new();
    scratch.init_with_mg();

    scratch.mg_ok(&["gc"]);
    assert!(pack_files(&scratch).is_empty(), "nothing to pack");
    assert!(
        !scratch.exists(".git/packed-refs"),
        "no refs -> mg gc must not create packed-refs"
    );
    assert_git_fsck_clean(&scratch, "empty repository after mg gc");
    assert_mg_fsck_clean(&scratch, &["fsck"], "empty repository after mg gc");
}

// ------------------------------------------------------- 全量 e2e：管线与历史

#[test]
fn e2e_plumbing_and_history_match_real_git() {
    let scratch = Scratch::new();
    scratch.init_with_mg();

    // init：真实 git 认得这个仓库。
    assert_eq!(scratch.git_ok(&["rev-parse", "--git-dir"]), ".git");
    assert_eq!(scratch.git_ok(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(scratch.git_ok(&["status", "--porcelain"]), "");
    assert_git_fsck_clean(&scratch, "fresh mg repository");

    scratch.write("a.txt", "alpha\n");
    scratch.write("dir/b.txt", "beta\n");
    scratch.write("c.txt", "gamma\n");

    // hash-object：与真 git 逐字节一致；`-w` 写出的对象 git 也读得到。
    let a_oid = scratch.git_ok(&["hash-object", "a.txt"]);
    assert_same(
        "hash-object a.txt",
        &scratch.mg_ok(&["hash-object", "a.txt"]),
        &a_oid,
    );
    assert_same(
        "hash-object -w a.txt",
        &scratch.mg_ok(&["hash-object", "-w", "a.txt"]),
        &a_oid,
    );
    assert_eq!(scratch.git_ok(&["cat-file", "-t", &a_oid]), "blob");

    // add：git 看到的 index（mode/oid/路径）必须与「git 自己 add」等价。
    scratch.mg_ok(&["add", "."]);
    let mut expected: Vec<(String, String)> = Vec::new();
    for rel in ["a.txt", "c.txt", "dir/b.txt"] {
        let oid = scratch.git_ok(&["hash-object", rel]);
        expected.push((rel.to_string(), format!("100644 {oid} 0\t{rel}")));
    }
    expected.sort();
    let expected_stage: Vec<String> = expected.into_iter().map(|(_, line)| line).collect();
    assert_same(
        "ls-files --stage after mg add",
        &scratch.git_ok(&["ls-files", "--stage"]),
        &expected_stage.join("\n"),
    );
    assert_same(
        "status --porcelain after mg add",
        &scratch.mg_ok(&["status", "--porcelain"]),
        &scratch.git_ok(&["status", "--porcelain"]),
    );
    assert_same(
        "diff --staged after mg add",
        &scratch.mg_ok(&["diff", "--staged"]),
        &scratch.git_ok(&["diff", "--staged"]),
    );

    // commit：mg 写的 commit/tree 必须与 git 的缓存树一致、fsck 干净。
    scratch.mg_ok(&["commit", "-m", "first"]);
    assert_same(
        "log --oneline after mg commit",
        &scratch.mg_ok(&["log", "--oneline"]),
        &scratch.git_ok(&["log", "--oneline"]),
    );
    assert_eq!(scratch.git_ok(&["status", "--porcelain"]), "");
    assert_eq!(
        scratch.git_ok(&["write-tree"]),
        scratch.git_ok(&["rev-parse", "HEAD^{tree}"]),
        "mg commit must leave a cache-tree that matches HEAD's tree"
    );
    assert_git_fsck_clean(&scratch, "after mg commit");

    // 工作区改动 + 未跟踪文件：mg diff / status 与 git 一致。
    scratch.write("a.txt", "alpha v2\n");
    scratch.write("untracked.txt", "u\n");
    assert_same(
        "diff (worktree)",
        &scratch.mg_ok(&["diff"]),
        &scratch.git_ok(&["diff"]),
    );
    assert_same(
        "status --porcelain (dirty)",
        &scratch.mg_ok(&["status", "--porcelain"]),
        &scratch.git_ok(&["status", "--porcelain"]),
    );
    scratch.mg_ok(&["add", "a.txt"]);
    scratch.mg_ok(&["commit", "-m", "second"]);
    assert_same(
        "log --oneline after 2nd commit",
        &scratch.mg_ok(&["log", "--oneline"]),
        &scratch.git_ok(&["log", "--oneline"]),
    );

    // tag：轻量 + annotated。
    scratch.mg_ok(&["tag", "v1"]);
    scratch.mg_ok(&["tag", "-a", "va", "-m", "annotated tag"]);
    assert_same(
        "tag --list",
        &scratch.mg_ok(&["tag", "--list"]),
        &scratch.git_ok(&["tag", "--list"]),
    );
    assert_eq!(scratch.git_ok(&["cat-file", "-t", "va"]), "tag");
    assert_eq!(scratch.git_ok(&["rev-parse", "v1^{commit}"]), scratch.git_ok(&["rev-parse", "HEAD"]));

    // branch + switch：HEAD 与工作区都要跟着走。
    scratch.mg_ok(&["branch", "feature"]);
    assert_same(
        "branch --list",
        &scratch.mg_ok(&["branch", "--list"]),
        &scratch.git_ok(&["branch", "--list"]),
    );
    scratch.mg_ok(&["switch", "feature"]);
    assert_eq!(scratch.git_ok(&["symbolic-ref", "HEAD"]), "refs/heads/feature");
    scratch.write("feature.txt", "f\n");
    scratch.mg_ok(&["add", "feature.txt"]);
    scratch.mg_ok(&["commit", "-m", "feature work"]);
    assert_same(
        "log --oneline on feature",
        &scratch.mg_ok(&["log", "--oneline"]),
        &scratch.git_ok(&["log", "--oneline"]),
    );
    scratch.mg_ok(&["switch", "main"]);
    assert_eq!(scratch.git_ok(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert!(
        !scratch.exists("feature.txt"),
        "switching back to main must remove feature's file"
    );
    assert_git_fsck_clean(&scratch, "after branch/switch");

    // reset：三种模式都以真实 git 看到的 HEAD / index / 工作区为准
    //（mg 的 rev 解析不支持 `~`/`^`，所以用全 oid 与分支名）。
    assert_eq!(scratch.git_ok(&["status", "--porcelain"]), "?? untracked.txt");
    let second = scratch.git_ok(&["rev-parse", "HEAD"]);
    let first = scratch.git_ok(&["rev-list", "--max-parents=0", "HEAD"]);

    scratch.mg_ok(&["reset", "--soft", &first]);
    assert_eq!(scratch.git_ok(&["rev-parse", "HEAD"]), first);
    assert_same(
        "status --porcelain after reset --soft",
        &scratch.mg_ok(&["status", "--porcelain"]),
        &scratch.git_ok(&["status", "--porcelain"]),
    );

    scratch.mg_ok(&["reset", "--mixed", &second]);
    assert_eq!(scratch.git_ok(&["rev-parse", "HEAD"]), second);
    assert_same(
        "status --porcelain after reset --mixed",
        &scratch.mg_ok(&["status", "--porcelain"]),
        &scratch.git_ok(&["status", "--porcelain"]),
    );

    scratch.mg_ok(&["reset", "--hard", &first]);
    assert_eq!(scratch.git_ok(&["rev-parse", "HEAD"]), first);
    assert_eq!(
        scratch.git_ok(&["status", "--porcelain"]),
        "?? untracked.txt",
        "reset --hard must not touch untracked files"
    );
    assert_eq!(scratch.read("a.txt"), "alpha\n");
    assert!(
        !scratch.exists("feature.txt"),
        "reset --hard must restore main's tree (feature.txt is not in it)"
    );

    // checkout：切分支 / 取回路径两种形态。
    scratch.mg_ok(&["checkout", "feature"]);
    assert_eq!(
        scratch.git_ok(&["symbolic-ref", "HEAD"]),
        "refs/heads/feature"
    );
    assert!(
        scratch.exists("feature.txt"),
        "checkout feature must materialize feature's tree"
    );
    scratch.mg_ok(&["checkout", "main"]);
    assert_eq!(scratch.git_ok(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    scratch.write("a.txt", "locally modified\n");
    scratch.mg_ok(&["checkout", "HEAD", "--", "a.txt"]);
    assert_eq!(
        scratch.read("a.txt"),
        "alpha\n",
        "checkout HEAD -- path must restore the file from HEAD"
    );
    assert_same(
        "status --porcelain after checkout",
        &scratch.mg_ok(&["status", "--porcelain"]),
        &scratch.git_ok(&["status", "--porcelain"]),
    );
    assert_git_fsck_clean(&scratch, "after checkout");
}

// ------------------------------------------------------- 全量 e2e：cat-file / rm

#[test]
fn e2e_cat_file_and_rm_match_real_git() {
    let scratch = Scratch::new();
    scratch.init_with_mg();
    scratch.write("a.txt", "alpha\n");
    scratch.write("dir/b.txt", "beta\n");
    scratch.mg_ok(&["add", "."]);
    scratch.mg_ok(&["commit", "-m", "first"]);
    scratch.mg_ok(&["tag", "-a", "va", "-m", "annotated tag"]);

    // cat-file：blob / tree / commit / tag 四种对象的 -t / -s / -p 都与真 git 逐字节一致。
    let objects = [
        scratch.git_ok(&["rev-parse", "HEAD:a.txt"]),
        scratch.git_ok(&["rev-parse", "HEAD^{tree}"]),
        scratch.git_ok(&["rev-parse", "HEAD"]),
        scratch.git_ok(&["rev-parse", "va"]),
    ];
    for oid in &objects {
        assert_same(
            &format!("cat-file -t {oid}"),
            &scratch.mg_ok(&["cat-file", "-t", oid]),
            &scratch.git_ok(&["cat-file", "-t", oid]),
        );
        assert_same(
            &format!("cat-file -s {oid}"),
            &scratch.mg_ok(&["cat-file", "-s", oid]),
            &scratch.git_ok(&["cat-file", "-s", oid]),
        );
        assert_same(
            &format!("cat-file -p {oid}"),
            &scratch.mg_ok(&["cat-file", "-p", oid]),
            &scratch.git_ok(&["cat-file", "-p", oid]),
        );
    }
    // 不存在的对象：与 git 一样响亮失败（不是静默成功）。
    let missing = "0".repeat(40);
    assert!(
        !scratch.mg(&["cat-file", "-p", &missing]).status.success(),
        "cat-file on a missing object must fail"
    );

    // rm：真实 git 看到的 index / 工作区必须与 `git rm` 等价。
    scratch.mg_ok(&["rm", "dir/b.txt"]);
    assert_eq!(
        scratch.git_ok(&["diff", "--cached", "--name-status"]),
        "D\tdir/b.txt"
    );
    assert!(!scratch.exists("dir/b.txt"));
    assert_same(
        "status --porcelain after mg rm",
        &scratch.mg_ok(&["status", "--porcelain"]),
        &scratch.git_ok(&["status", "--porcelain"]),
    );
    scratch.mg_ok(&["commit", "-m", "drop b"]);
    assert_eq!(scratch.git_ok(&["ls-files"]), "a.txt");
    assert_eq!(scratch.git_ok(&["cat-file", "-t", "HEAD^{tree}"]), "tree");
    assert_git_fsck_clean(&scratch, "after mg rm + commit");
}

// ------------------------------------------------------- 全量 e2e：合并
#[test]
fn e2e_merge_conflict_in_a_nested_path_matches_real_git() {
    let scratch = Scratch::new();
    scratch.init_with_mg();
    scratch.write("dir/b.txt", "beta\n");
    scratch.write("a.txt", "alpha\n");
    scratch.mg_ok(&["add", "."]);
    scratch.mg_ok(&["commit", "-m", "base"]);

    scratch.mg_ok(&["switch", "-c", "side", "main"]);
    scratch.write("dir/b.txt", "side version\n");
    scratch.mg_ok(&["add", "dir/b.txt"]);
    scratch.mg_ok(&["commit", "-m", "side edits b"]);

    scratch.mg_ok(&["switch", "main"]);
    scratch.write("dir/b.txt", "main version\n");
    scratch.mg_ok(&["add", "dir/b.txt"]);
    scratch.mg_ok(&["commit", "-m", "main edits b"]);

    // 冲突：非 0 退出 + 冲突状态与真实 git 完全一致。
    let out = scratch.mg(&["merge", "side"]);
    assert!(!out.status.success(), "a conflicting merge must fail");
    assert!(
        !String::from_utf8_lossy(&out.stderr).is_empty(),
        "a conflicting merge must explain itself on stderr"
    );
    assert_same(
        "status --porcelain (conflicted)",
        &scratch.mg_ok(&["status", "--porcelain"]),
        &scratch.git_ok(&["status", "--porcelain"]),
    );
    assert_eq!(scratch.git_ok(&["status", "--porcelain"]), "UU dir/b.txt");
    assert_eq!(
        scratch.git_ok(&["ls-files", "-u"]).lines().count(),
        3,
        "a content conflict must stage base/ours/theirs"
    );
    let conflicted = scratch.read("dir/b.txt");
    for marker in ["<<<<<<< HEAD", "=======", ">>>>>>> side"] {
        assert!(
            conflicted.contains(marker),
            "conflict markers must be written to the worktree, got {conflicted:?}"
        );
    }
    // 合并进行中：index 里有冲突时不能直接提交。
    let blocked = scratch.mg(&["commit", "-m", "premature"]);
    assert!(
        !blocked.status.success(),
        "committing with unresolved conflicts must fail"
    );

    // 解决冲突 + 提交：两个 parent、fsck 干净、历史与 git 一致。
    scratch.write("dir/b.txt", "resolved\n");
    scratch.mg_ok(&["add", "dir/b.txt"]);
    scratch.mg_ok(&["commit", "-m", "merge side"]);
    assert_eq!(scratch.git_ok(&["status", "--porcelain"]), "");
    assert_eq!(
        scratch.git_ok(&["rev-list", "--parents", "-1", "HEAD"]).split_whitespace().count(),
        3,
        "the merge commit must have two parents"
    );
    assert_same(
        "log --oneline after the merge",
        &scratch.mg_ok(&["log", "--oneline"]),
        &scratch.git_ok(&["log", "--oneline"]),
    );
    assert_eq!(scratch.read("dir/b.txt"), "resolved\n");
    assert_git_fsck_clean(&scratch, "after the merge commit");
}

#[test]
fn e2e_clean_merge_then_fsck_and_gc_close_out() {
    let scratch = Scratch::new();
    scratch.init_with_mg();
    scratch.write("dir/b.txt", "beta\n");
    scratch.mg_ok(&["add", "."]);
    scratch.mg_ok(&["commit", "-m", "base"]);

    scratch.mg_ok(&["switch", "-c", "side", "main"]);
    scratch.write("side.txt", "side\n");
    scratch.mg_ok(&["add", "side.txt"]);
    scratch.mg_ok(&["commit", "-m", "side work"]);

    scratch.mg_ok(&["switch", "main"]);
    scratch.write("main.txt", "main\n");
    scratch.mg_ok(&["add", "main.txt"]);
    scratch.mg_ok(&["commit", "-m", "main work"]);

    // 无冲突的自动合并（两侧各加了文件）。
    scratch.mg_ok(&["merge", "side"]);
    assert_eq!(scratch.git_ok(&["status", "--porcelain"]), "");
    assert_eq!(
        scratch.git_ok(&["rev-list", "--parents", "-1", "HEAD"]).split_whitespace().count(),
        3
    );
    assert!(scratch.exists("side.txt") && scratch.exists("main.txt"));
    assert_same(
        "log --oneline after the clean merge",
        &scratch.mg_ok(&["log", "--oneline"]),
        &scratch.git_ok(&["log", "--oneline"]),
    );
    assert_git_fsck_clean(&scratch, "after the clean merge");

    // 收口：fsck → gc → 真实 git 复查。
    let before_log = scratch.git_ok(&["log", "--all", "--oneline"]);
    let before_refs = scratch.git_ok(&["show-ref"]);
    assert_mg_fsck_clean(&scratch, &["fsck"], "before gc");
    assert_mg_fsck_clean(&scratch, &["fsck", "--full"], "before gc (--full)");

    scratch.mg_ok(&["gc"]);
    assert_eq!(count_objects(&scratch)["count"], 0);
    assert_mg_fsck_clean(&scratch, &["fsck"], "after gc");
    assert_git_fsck_clean(&scratch, "after gc");
    assert_same(
        "log --all --oneline after gc",
        &scratch.git_ok(&["log", "--all", "--oneline"]),
        &before_log,
    );
    assert_same(
        "show-ref after gc",
        &scratch.git_ok(&["show-ref"]),
        &before_refs,
    );
    assert_eq!(scratch.git_ok(&["status", "--porcelain"]), "");
}

// ------------------------------------------------------- 全量 e2e：传输（clone/fetch/pull）

/// `mg` 建的 seed 仓库 + 真实 git 建的 bare 远端（远端主分支指向 seed 的 main）。
/// `nested` 决定夹具里有没有子目录（`mg pull` 的快进受 merge.rs 的已知缺陷限制，
/// 见文件末尾那条 `#[ignore]` 用例，所以 pull 用扁平夹具）。
fn bare_origin(workspace: &Scratch, nested: bool) -> (PathBuf, PathBuf) {
    let seed = workspace.join("seed");
    fs::create_dir_all(&seed).expect("seed dir");
    assert!(run_ok(&seed, true, &["init", "."]).contains("Initialized"));
    fs::write(seed.join("a.txt"), "alpha\n").expect("a.txt");
    if nested {
        fs::create_dir_all(seed.join("dir")).expect("dir");
        fs::write(seed.join("dir/b.txt"), "beta\n").expect("dir/b.txt");
    }
    run_ok(&seed, true, &["add", "."]);
    run_ok(&seed, true, &["commit", "-m", "first"]);
    run_ok(&seed, true, &["tag", "v1"]);

    let bare = workspace.join("origin.git");
    run_ok(
        workspace.path(),
        false,
        &["init", "--bare", bare.to_str().unwrap()],
    );
    let url = format!("file://{}", bare.display());
    run_ok(&seed, false, &["remote", "add", "origin", &url]);
    run_ok(&seed, false, &["push", "-q", "origin", "main"]);
    run_ok(&seed, false, &["push", "-q", "origin", "--tags"]);
    (bare, seed)
}

#[test]
fn e2e_clone_and_fetch_against_a_bare_git_remote() {
    let workspace = Scratch::new();
    let (bare, seed) = bare_origin(&workspace, true);
    let url = format!("file://{}", bare.display());

    // clone：新仓库必须能被真实 git 认，引用/历史/工作区都要对上。
    let dest = workspace.join("copy");
    run_ok(
        workspace.path(),
        true,
        &["clone", &url, dest.to_str().unwrap()],
    );
    assert_git_fsck_clean_in(&dest, "the clone");
    assert_eq!(run_ok(&dest, false, &["status", "--porcelain"]), "");
    assert_eq!(
        run_ok(&dest, false, &["tag", "--list"]),
        run_ok(&bare, false, &["tag", "--list"]),
        "clone must carry the tags over"
    );
    assert_eq!(
        run_ok(&dest, false, &["rev-parse", "refs/heads/main"]),
        run_ok(&bare, false, &["rev-parse", "refs/heads/main"])
    );
    assert_eq!(
        run_ok(&dest, false, &["rev-parse", "refs/tags/v1"]),
        run_ok(&bare, false, &["rev-parse", "refs/tags/v1"])
    );
    assert_eq!(
        run_ok(&dest, false, &["rev-parse", "refs/remotes/origin/main"]),
        run_ok(&bare, false, &["rev-parse", "refs/heads/main"]),
        "clone must record the remote-tracking ref"
    );
    assert_eq!(
        run_ok(&dest, false, &["log", "--oneline"]),
        run_ok(&bare, false, &["log", "--oneline"])
    );
    assert_eq!(
        fs::read_to_string(dest.join("dir/b.txt")).expect("cloned file"),
        "beta\n"
    );

    // 远端前进（真实 git 推），然后 mg fetch。
    fs::write(seed.join("c.txt"), "gamma\n").expect("c.txt");
    run_ok(&seed, false, &["add", "-A"]);
    run_ok(&seed, false, &["commit", "-qm", "on the origin"]);
    run_ok(&seed, false, &["push", "-q", "origin", "main"]);

    run_ok(&dest, true, &["fetch", "origin"]);
    assert_eq!(
        run_ok(&dest, false, &["rev-parse", "refs/remotes/origin/main"]),
        run_ok(&bare, false, &["rev-parse", "refs/heads/main"]),
        "fetch must update refs/remotes/origin/main"
    );
    assert_eq!(
        run_ok(&dest, false, &["rev-parse", "HEAD"]),
        run_ok(&dest, false, &["rev-parse", "refs/heads/main"]),
        "fetch must not move the local branch"
    );
    assert_git_fsck_clean_in(&dest, "after fetch");
}

#[test]
fn e2e_pull_fast_forwards_from_a_bare_git_remote() {
    let workspace = Scratch::new();
    let (bare, seed) = bare_origin(&workspace, false);
    let url = format!("file://{}", bare.display());

    let dest = workspace.join("copy");
    run_ok(
        workspace.path(),
        true,
        &["clone", &url, dest.to_str().unwrap()],
    );
    assert_eq!(
        run_ok(&dest, false, &["rev-parse", "HEAD"]),
        run_ok(&bare, false, &["rev-parse", "refs/heads/main"])
    );

    // 远端前进（真实 git 推），mg pull 必须快进到远端。
    fs::write(seed.join("c.txt"), "gamma\n").expect("c.txt");
    run_ok(&seed, false, &["add", "-A"]);
    run_ok(&seed, false, &["commit", "-qm", "on the origin"]);
    run_ok(&seed, false, &["push", "-q", "origin", "main"]);

    run_ok(&dest, true, &["pull", "origin"]);
    assert_eq!(
        run_ok(&dest, false, &["log", "--oneline"]),
        run_ok(&bare, false, &["log", "--oneline"])
    );
    assert_eq!(run_ok(&dest, false, &["status", "--porcelain"]), "");
    assert_eq!(
        fs::read_to_string(dest.join("c.txt")).expect("pulled file"),
        "gamma\n"
    );
    assert_git_fsck_clean_in(&dest, "after pull");
}

// ------------------------------------------------------- 已知缺口（显式标注）

/// `mg clone` 在**远端没有 tag**时直接失败；真实 git 对同样的仓库正常克隆。
///
/// 最小复现（`origin` 由真实 git 建、无 tag）：
/// ```text
/// git init -q origin; cd origin; echo a > a.txt; git add .; git commit -qm one
/// mg clone file://$PWD/origin /tmp/copy
///   -> mg: fatal: couldn't find remote ref refs/tags/*        (exit 1)
/// ```
/// 修好 `src/cli/clone.rs` / `src/transport/local.rs` 里的 refspec 处理（T13/T14 范围）后，
/// 删掉 `#[ignore]` 即可作为回归用例。
#[test]
#[ignore = "blocked outside T15's scope (T13/T14): mg clone fails when the origin has no tags \
            (\"couldn't find remote ref refs/tags/*\"); real git clones that repository fine"]
fn clone_of_an_origin_without_tags_succeeds() {
    let workspace = Scratch::new();
    let origin = workspace.join("origin");
    fs::create_dir_all(&origin).expect("origin dir");
    run_ok(&origin, false, &["init", "--quiet", "."]);
    fs::write(origin.join("a.txt"), "alpha\n").expect("a.txt");
    run_ok(&origin, false, &["add", "."]);
    run_ok(&origin, false, &["commit", "-qm", "one"]);

    let dest = workspace.join("copy");
    let url = format!("file://{}", origin.display());
    run_ok(
        workspace.path(),
        true,
        &["clone", &url, dest.to_str().unwrap()],
    );
    assert_eq!(
        run_ok(&dest, false, &["log", "--oneline"]),
        run_ok(&origin, false, &["log", "--oneline"])
    );
    assert_eq!(run_ok(&dest, false, &["status", "--porcelain"]), "");
    assert_git_fsck_clean_in(&dest, "the clone of a tagless origin");
}

/// `mg push` 有两个与真实 git 不一致的拒绝：bare 远端被当成「非 bare」，
/// 以及**新建远端分支**被当成「远端引用被移动」。真实 git 在同一夹具上两者都成功。
///
/// 最小复现：
/// ```text
/// git init --bare /tmp/o.git                       # bare 远端
/// mg clone file:///tmp/o.git /tmp/c && cd /tmp/c   # 需要远端有 tag（见上一个用例的缺陷）
/// echo x > x.txt; mg add x.txt; mg commit -m x
/// mg push origin main
///   -> refs/heads/main: branch is currently checked out   (exit 1；但远端是 bare)
/// git push origin main                                -> 成功（acff8b8..8c74511）
/// mg switch -c feature main; ...; mg push origin feature
///   -> refs/heads/feature: cannot lock ref: the remote ref moved   (exit 1；真实 git 会新建该 ref)
/// ```
/// 根因线索（供 T13/T14 参考）：`src/transport/local.rs::checked_out_branch()` 只判断
/// `repo.workdir().is_none()`，而 `Repo::discover/open_git_dir` 对 bare 仓库也会把
/// `workdir` 设成 Some（应看 `core.bare`）；`receive_pack` 的 CAS 校验直接把
/// 「远端没有这个 ref（`None`）」判成「引用被移动」，没有放行「old = oid 全 0 的新建分支」。
#[test]
#[ignore = "blocked outside T15's scope (T13/T14): mg push rejects bare remotes and new branches; \
            real git performs both pushes on the same fixture"]
fn push_to_a_bare_remote_updates_and_creates_branches() {
    let workspace = Scratch::new();
    let (bare, _seed) = bare_origin(&workspace, true);
    let url = format!("file://{}", bare.display());

    let dest = workspace.join("copy");
    run_ok(
        workspace.path(),
        true,
        &["clone", &url, dest.to_str().unwrap()],
    );
    fs::write(dest.join("x.txt"), "x\n").expect("x.txt");
    run_ok(&dest, true, &["add", "x.txt"]);
    run_ok(&dest, true, &["commit", "-m", "from the clone"]);
    run_ok(&dest, true, &["push", "origin", "main"]);
    assert_eq!(
        run_ok(&bare, false, &["rev-parse", "refs/heads/main"]),
        run_ok(&dest, false, &["rev-parse", "HEAD"])
    );

    // 新建远端分支：真实 git 会创建 refs/heads/feature。
    run_ok(&dest, true, &["switch", "-c", "feature", "main"]);
    fs::write(dest.join("f.txt"), "f\n").expect("f.txt");
    run_ok(&dest, true, &["add", "f.txt"]);
    run_ok(&dest, true, &["commit", "-m", "feature work"]);
    run_ok(&dest, true, &["push", "origin", "feature"]);
    assert_eq!(
        run_ok(&bare, false, &["rev-parse", "refs/heads/feature"]),
        run_ok(&dest, false, &["rev-parse", "HEAD"])
    );
    assert_git_fsck_clean_in(&bare, "the bare origin after both pushes");
}

/// 真实 git：仓库里只要有嵌套路径，`mg merge` 的快进路径就坏掉。
///
/// 根因不在 T15 的写作用域里：`src/cli/merge.rs::fast_forward` 把**已展平**的路径
/// 当成 tree 条目名塞回一个 `Tree`（`flat_tree()`），再交给 `materialize_tree()`；
/// `worktree/materialize.rs::validate_name` 正确地拒绝了含 `/` 的名字。最小复现：
///
/// ```text
/// mg init .; mkdir dir; echo beta > dir/b.txt; mg add .; mg commit -m c1
/// mg switch -c side main; echo s > s.txt; mg add s.txt; mg commit -m c2
/// mg switch main; mg merge side
///   -> mg: fatal: corrupt tree entry name: "dir/b.txt"
/// ```
///
/// 一旦那个文件修好（正确的做法是直接把展平条目交给 `materialize::apply_tree`，
/// 或先 `build_tree` 还原成嵌套 tree），删掉下面的 `#[ignore]` 即可作为回归用例。
#[test]
#[ignore = "blocked outside T15's write scope: src/cli/merge.rs::fast_forward() hands a \
            flattened-path Tree to materialize_tree(); see the doc comment for the minimal repro"]
fn e2e_fast_forward_merge_in_a_repository_with_subdirectories() {
    let scratch = Scratch::new();
    scratch.init_with_mg();
    scratch.write("dir/b.txt", "beta\n");
    scratch.mg_ok(&["add", "."]);
    scratch.mg_ok(&["commit", "-m", "base"]);

    scratch.mg_ok(&["switch", "-c", "side", "main"]);
    scratch.write("dir/b.txt", "beta v2\n");
    scratch.mg_ok(&["add", "dir/b.txt"]);
    scratch.mg_ok(&["commit", "-m", "side edits b"]);

    scratch.mg_ok(&["switch", "main"]);
    let out = scratch.mg(&["merge", "side"]);
    assert!(
        out.status.success(),
        "fast-forward merge must succeed, got {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(scratch.read("dir/b.txt"), "beta v2\n");
    assert_eq!(scratch.git_ok(&["status", "--porcelain"]), "");
    assert_eq!(
        scratch.git_ok(&["rev-parse", "HEAD"]),
        scratch.git_ok(&["rev-parse", "refs/heads/side"]),
        "fast-forward must move the branch to side's commit"
    );
    assert_git_fsck_clean(&scratch, "after the fast-forward merge");
}
