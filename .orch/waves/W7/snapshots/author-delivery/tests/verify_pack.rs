//! V9 —— 独立验证 T9（packfile 读取：`.idx` v2 / 对象头 / OFS_DELTA / REF_DELTA）。
//!
//! 判定真值只来自**真实 git 进程**（`cat-file` / `verify-pack` / `count-objects`）
//! 与文件系统。本文件不硬编码任何 oid 或期望字节——唯一的常量是 `.idx` / `.pack`
//! 的格式常量（magic、版本、表布局），那是格式定义本身。
//!
//! 与作者用例的独立性：
//! * 不引用作者在 `src/odb/pack/*.rs` 里的 `gitkit` 测试台，自带一套 git 调用与
//!   `git verify-pack -v` 解析；
//! * 用 `git verify-pack -v` 给出的对象偏移当**外部真值**驱动的独立 pack 扫描器
//!   （自证：对象类型/长度与 verify-pack 一致，且所有对象首尾相接、正好耗尽 pack
//!   主体），从而能做「改写真实 delta 负载」这类定向反例；
//! * 反例不只看 `Err`，还检查错误种类是 `Corrupt`（不是 `Unsupported`，更不是
//!   `ObjectNotFound` 这种伪装的假绿），并核对错误落在预期的检查分支上。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use minigit::error::Error;
use minigit::odb::pack::{PackFile, PackIndex, PackSet};
use minigit::odb::Odb;
use minigit::oid::Oid;
use minigit::repo::Repo;

// ---------------------------------------------------------------------------
// 真 git 调用（隔离配置/身份，绝不 export 到共享 shell）
// ---------------------------------------------------------------------------

fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "V9")
        .env("GIT_AUTHOR_EMAIL", "v9@example.com")
        .env("GIT_COMMITTER_NAME", "V9")
        .env("GIT_COMMITTER_EMAIL", "v9@example.com")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES");
    for global in [
        "-c",
        "init.defaultBranch=main",
        "-c",
        "core.autocrlf=false",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "tag.gpgsign=false",
    ] {
        cmd.arg(global);
    }
    cmd.args(args);
    cmd
}

/// git 必须存在：真值只能来自它，缺了它不是「跳过」，而是验证不成立。
fn require_git() {
    let out = Command::new("git").arg("--version").output();
    assert!(
        out.map(|out| out.status.success()).unwrap_or(false),
        "this verification suite requires a real `git`; none is available"
    );
}

fn run(dir: &Path, args: &[&str]) -> Output {
    git_command(dir, args)
        .output()
        .unwrap_or_else(|err| panic!("failed to spawn git {args:?}: {err}"))
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn git_bytes(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = run(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn git_stdin(dir: &Path, args: &[&str], input: &[u8]) -> Vec<u8> {
    let mut cmd = git_command(dir, args);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("failed to spawn git");
    child
        .stdin
        .take()
        .expect("git stdin")
        .write_all(input)
        .expect("write git stdin");
    let out = child.wait_with_output().expect("git wait");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

// ---------------------------------------------------------------------------
// 真值读取
// ---------------------------------------------------------------------------

struct GitObject {
    oid: Oid,
    kind: String,
    size: u64,
}

/// `git cat-file --batch-all-objects --batch-check`：git 眼里的对象全集。
fn git_objects(dir: &Path) -> Vec<GitObject> {
    let text = git(
        dir,
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname) %(objecttype) %(objectsize)",
        ],
    );
    let mut objects = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 3, "unexpected --batch-check line: {line:?}");
        objects.push(GitObject {
            oid: Oid::from_hex(fields[0]).expect("oid"),
            kind: fields[1].to_string(),
            size: fields[2].parse().expect("size"),
        });
    }
    objects.sort_by_key(|object| object.oid);
    // 同一 oid 出现在多个 pack 里时 git 只报一次（实测 v2.55）；这里再断言一次，
    // 以免哪天口径漂移后本套验证的真值悄悄变了。
    let listed: Vec<Oid> = objects.iter().map(|object| object.oid).collect();
    let mut unique = listed.clone();
    unique.dedup();
    assert_eq!(listed, unique, "git listed the same oid twice");
    assert!(!objects.is_empty(), "fixture produced no objects");
    objects
}

fn object_of(objects: &[GitObject], oid: Oid) -> &GitObject {
    objects
        .iter()
        .find(|object| object.oid == oid)
        .unwrap_or_else(|| panic!("{oid} is missing from the git object list"))
}

/// 逐字节真值：`git cat-file --batch`（二进制安全，一次进程）。
fn cat_file_batch(dir: &Path, objects: &[GitObject]) -> Vec<Vec<u8>> {
    let mut input = Vec::new();
    for object in objects {
        input.extend_from_slice(object.oid.to_hex().as_bytes());
        input.push(b'\n');
    }
    let raw = git_stdin(dir, &["cat-file", "--batch"], &input);
    let mut payloads = Vec::with_capacity(objects.len());
    let mut pos = 0usize;
    for object in objects {
        let newline = raw[pos..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| pos + offset)
            .unwrap_or_else(|| panic!("git --batch stopped early at {pos}"));
        let header = String::from_utf8_lossy(&raw[pos..newline]).to_string();
        pos = newline + 1;
        let fields: Vec<&str> = header.split(' ').collect();
        assert_eq!(fields.len(), 3, "unexpected --batch header: {header:?}");
        assert_eq!(fields[0], object.oid.to_hex(), "batch order drifted");
        assert_eq!(fields[1], object.kind, "{}: type drifted", object.oid);
        let size: usize = fields[2].parse().expect("size");
        assert_eq!(size as u64, object.size, "{}: size drifted", object.oid);
        payloads.push(raw[pos..pos + size].to_vec());
        pos += size;
        assert_eq!(
            raw[pos], b'\n',
            "{}: batch payload not newline framed",
            object.oid
        );
        pos += 1;
    }
    assert_eq!(pos, raw.len(), "git --batch emitted extra bytes");
    payloads
}

/// `git cat-file <type> <oid>` —— 任务书指定的真值来源，对每个对象（不抽样）。
fn cat_file_direct(dir: &Path, object: &GitObject) -> Vec<u8> {
    git_bytes(dir, &["cat-file", &object.kind, &object.oid.to_hex()])
}

/// `git count-objects -v` 的 loose 数量：用来确认「git 的对象全集 == pack 里的对象」，
/// 否则拿 `--batch-all-objects` 当 pack 集合真值就是错的。
fn loose_object_count(dir: &Path) -> u64 {
    let text = git(dir, &["count-objects", "-v"]);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("count:") {
            return rest.trim().parse().expect("loose count");
        }
    }
    panic!("git count-objects -v printed no count line");
}

/// `git verify-pack -v <idx>` 里的对象行。
struct PackEntry {
    oid: Oid,
    kind: String,
    /// delta 对象时这是 **delta 数据**的长度（git 字段语义），不是还原后的对象大小。
    size: u64,
    size_in_pack: u64,
    offset: u64,
    depth: usize,
    base: Option<Oid>,
}

fn parse_verify_pack(text: &str) -> Vec<PackEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5
            || fields[0].len() != 40
            || !fields[0].chars().all(|c| c.is_ascii_hexdigit())
        {
            continue; // "non delta: N objects" / "chain length = k: N objects" / "<idx>: ok"
        }
        entries.push(PackEntry {
            oid: Oid::from_hex(fields[0]).expect("verify-pack oid"),
            kind: fields[1].to_string(),
            size: fields[2].parse().expect("verify-pack size"),
            size_in_pack: fields[3].parse().expect("verify-pack size in pack"),
            offset: fields[4].parse().expect("verify-pack offset"),
            depth: fields
                .get(5)
                .map(|d| d.parse().expect("depth"))
                .unwrap_or(0),
            base: fields.get(6).map(|b| Oid::from_hex(b).expect("base oid")),
        });
    }
    assert!(!entries.is_empty(), "git verify-pack listed no objects");
    entries
}

fn verify_pack(dir: &Path, idx: &Path) -> Vec<PackEntry> {
    parse_verify_pack(&git(
        dir,
        &["verify-pack", "-v", &idx.display().to_string()],
    ))
}

fn verify_pack_raw(dir: &Path, idx: &Path) -> String {
    git(dir, &["verify-pack", "-v", &idx.display().to_string()])
}

fn pack_indices(dir: &Path) -> Vec<PathBuf> {
    let pack_dir = dir.join(".git/objects/pack");
    let mut paths: Vec<PathBuf> = fs::read_dir(&pack_dir)
        .expect("pack dir")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "idx"))
        .collect();
    paths.sort();
    paths
}

// ---------------------------------------------------------------------------
// 独立 pack 扫描器：以 `git verify-pack -v` 的偏移为外部真值，自证首尾相接
// ---------------------------------------------------------------------------

struct RawObject {
    offset: u64,
    ty: u8,
    /// 头里「负偏移 / base oid」那一段的字节数（非 delta 为 0）。
    base_varint_len: usize,
    data_pos: usize,
    compressed_len: usize,
}

fn scan_pack(pack: &[u8], entries: &[PackEntry]) -> Vec<RawObject> {
    assert!(pack.starts_with(b"PACK"), "not a pack");
    let count = u32::from_be_bytes([pack[8], pack[9], pack[10], pack[11]]) as usize;
    assert_eq!(count, entries.len(), "pack header count vs verify-pack");
    let bound = pack.len() - 20;

    let mut ordered: Vec<&PackEntry> = entries.iter().collect();
    ordered.sort_by_key(|entry| entry.offset);

    let mut objects = Vec::with_capacity(count);
    for (i, entry) in ordered.iter().enumerate() {
        let start = usize::try_from(entry.offset).expect("offset fits");
        assert!(
            (12..bound).contains(&start),
            "verify-pack offset {} is not inside the pack body",
            entry.offset
        );
        let mut pos = start;
        let mut byte = pack[pos];
        pos += 1;
        let ty = (byte >> 4) & 0x07;
        while byte & 0x80 != 0 {
            byte = pack[pos];
            pos += 1;
        }
        let size_end = pos;
        if ty == 6 {
            let mut byte = pack[pos];
            pos += 1;
            while byte & 0x80 != 0 {
                byte = pack[pos];
                pos += 1;
            }
        } else if ty == 7 {
            pos += 20;
        }
        let base_varint_len = if ty == 6 || ty == 7 {
            pos - size_end
        } else {
            0
        };
        let (payload, consumed) = minigit::zlib::inflate_prefix(&pack[pos..bound])
            .unwrap_or_else(|err| panic!("offset {}: zlib: {err}", entry.offset));

        // 交叉校验：非 delta 的类型/长度必须与 verify-pack 一致，delta 必须是类型 6/7。
        // 这把「我的扫描器」钉在 git 的真值上，后面的定向反例才可信。
        if entry.depth == 0 {
            let expected = match entry.kind.as_str() {
                "commit" => 1,
                "tree" => 2,
                "blob" => 3,
                "tag" => 4,
                other => panic!("unexpected type column {other:?}"),
            };
            assert_eq!(ty, expected, "offset {}: type mismatch", entry.offset);
            assert_eq!(
                payload.len() as u64,
                entry.size,
                "offset {}: object size mismatch",
                entry.offset
            );
        } else {
            assert!(
                ty == 6 || ty == 7,
                "offset {}: delta object must have type 6/7",
                entry.offset
            );
            assert_eq!(
                payload.len() as u64,
                entry.size,
                "offset {}: delta data size mismatch",
                entry.offset
            );
        }

        let end = pos + consumed;
        let next = ordered
            .get(i + 1)
            .map(|next| usize::try_from(next.offset).expect("offset fits"))
            .unwrap_or(bound);
        assert_eq!(
            end, next,
            "object at offset {} does not end where the next object starts",
            entry.offset
        );
        objects.push(RawObject {
            offset: entry.offset,
            ty,
            base_varint_len,
            data_pos: pos,
            compressed_len: consumed,
        });
    }
    objects
}

fn raw_of(raw: &[RawObject], offset: u64) -> &RawObject {
    raw.iter()
        .find(|object| object.offset == offset)
        .unwrap_or_else(|| panic!("no scanned object at offset {offset}"))
}

// ---------------------------------------------------------------------------
// 变异/反例工具
// ---------------------------------------------------------------------------

fn sha1_of(bytes: &[u8]) -> [u8; 20] {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    let mut out = [0u8; 20];
    out.copy_from_slice(&hasher.finalize());
    out
}

/// 重算 `.idx` 尾部（它自己那个）的 sha1，好让反例打结构校验而不是校验和。
fn resign_index(bytes: &mut [u8]) {
    let len = bytes.len();
    let sum = sha1_of(&bytes[..len - 20]);
    bytes[len - 20..].copy_from_slice(&sum);
}

/// 重算 `.pack` 尾部的全文件 sha1（同上）。
fn resign_pack(bytes: &mut [u8]) {
    let len = bytes.len();
    let sum = sha1_of(&bytes[..len - 20]);
    bytes[len - 20..].copy_from_slice(&sum);
}

const IDX_HEADER_LEN: usize = 8;
const IDX_FANOUT_LEN: usize = 256 * 4;

/// 解析 `.idx` v2 的表位置：`(对象数, oid 表起点, offset 表起点)`。
fn idx_table_positions(idx: &[u8]) -> (usize, usize, usize) {
    let tail = IDX_HEADER_LEN + 255 * 4;
    let n = u32::from_be_bytes([idx[tail], idx[tail + 1], idx[tail + 2], idx[tail + 3]]) as usize;
    let oid_table = IDX_HEADER_LEN + IDX_FANOUT_LEN;
    let crc_table = oid_table + n * 20;
    let offset_table = crc_table + n * 4;
    (n, oid_table, offset_table)
}

/// git 把 `.pack` / `.idx` 写成 0444，直接 `fs::write` 会 PermissionDenied。
fn force_write(path: &Path, bytes: &[u8]) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("remove {}: {err}", path.display()),
    }
    fs::write(path, bytes).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
}

fn write_u32be(bytes: &mut [u8], pos: usize, value: u32) {
    bytes[pos..pos + 4].copy_from_slice(&value.to_be_bytes());
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            return out;
        }
    }
}

/// pack 对象头（类型 + size 的紧凑 varint），不含 delta 的 base 段。
fn pack_object_header(ty: u8, size: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut byte = ((ty & 0x07) << 4) | (size & 0x0f) as u8;
    let mut rest = size >> 4;
    while rest != 0 {
        out.push(byte | 0x80);
        byte = (rest & 0x7f) as u8;
        rest >>= 7;
    }
    out.push(byte);
    out
}

fn corrupt_detail(err: Error) -> String {
    match err {
        Error::Corrupt { what, detail } => format!("{what}: {detail}"),
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 夹具：三代相似文本（逼出多级 delta 链）+ 二进制 + 空文件 + 注解 tag
// ---------------------------------------------------------------------------

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        out.push((state >> 33) as u8);
    }
    out
}

fn write_generation(dir: &Path, generation: usize) {
    let text = dir.join("text");
    fs::create_dir_all(&text).unwrap();
    let base: Vec<String> = (0..120)
        .map(|i| format!("line {i:03}: the quick brown fox jumps over the lazy dog\n"))
        .collect();
    for file in 0..20usize {
        let mut lines = base.clone();
        for edit in 0..=(generation * 3 + file % 5) {
            let idx = (file * 11 + edit * 17) % lines.len();
            lines[idx] =
                format!("line {idx:03}: file {file} generation {generation} rewrote this\n");
        }
        fs::write(text.join(format!("f{file:02}.txt")), lines.concat()).unwrap();
    }
    if generation == 0 {
        fs::write(
            text.join("f00-copy.txt"),
            fs::read(text.join("f00.txt")).unwrap(),
        )
        .unwrap();
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("empty"), b"").unwrap();
        fs::write(bin.join("noise.bin"), noise(300_000, 0x9E37_79B9_7F4A_7C15)).unwrap();
        fs::write(bin.join("repeat.bin"), b"ABCDEFGHIJKLMNOP".repeat(25_000)).unwrap();
        let deep = dir.join("deep/nested/path");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("leaf.txt"), b"deep leaf\n").unwrap();
    } else {
        let mut repeat = fs::read(dir.join("bin/repeat.bin")).unwrap();
        for byte in repeat.iter_mut().skip(123_456).take(16) {
            *byte = b'z';
        }
        fs::write(dir.join("bin/repeat.bin"), repeat).unwrap();
    }
}

/// 三代提交 + 一个注解 tag（tag 对象也要能在 pack 里被读出）。
fn fixture(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    for generation in 0..3 {
        write_generation(dir, generation);
        git(dir, &["add", "-A"]);
        git(
            dir,
            &["commit", "-q", "-m", &format!("v9 generation {generation}")],
        );
    }
    git(
        dir,
        &["tag", "-a", "v9-tag", "-m", "annotated tag object for V9"],
    );
}

// ---------------------------------------------------------------------------
// 证据落盘
// ---------------------------------------------------------------------------

fn scratch_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".orch/waves/W3/T9-pack/verify-scratch");
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn record(label: &str, lines: &[String]) {
    let text = format!("{}\n", lines.join("\n"));
    eprint!("{text}");
    let path = scratch_dir().join(format!("v9-{label}.txt"));
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open evidence file");
    file.write_all(text.as_bytes()).expect("write evidence");
}

// ---------------------------------------------------------------------------
// 核心断言：PackSet / PackIndex / PackFile 与真实 git 逐字节对拍
// ---------------------------------------------------------------------------

fn check_pack_set_against_git(dir: &Path, label: &str) -> Vec<String> {
    let mut evidence = vec![format!("[{label}] dir={}", dir.display())];

    assert_eq!(
        loose_object_count(dir),
        0,
        "[{label}] fixture still has loose objects: `git cat-file --batch-all-objects` would then \
         not be a pack-only truth source"
    );

    let objects = git_objects(dir);
    let truth = cat_file_batch(dir, &objects);
    let expected_oids: Vec<Oid> = objects.iter().map(|object| object.oid).collect();

    let repo = Repo::discover(dir).expect("discover repo");
    let set = PackSet::open(&repo).expect("PackSet::open");
    assert!(!set.indices.is_empty(), "[{label}] no idx was loaded");

    // (1) oid 集合与数量。
    let got = set.iter_oids().expect("iter_oids");
    assert_eq!(
        got.len(),
        expected_oids.len(),
        "[{label}] PackSet::iter_oids count differs from git"
    );
    assert_eq!(
        got, expected_oids,
        "[{label}] PackSet::iter_oids set differs from `git cat-file --batch-all-objects`"
    );

    // (2) pack 作用域真值（verify-pack）也必须与 git 全集一致（loose 已为 0）。
    let indices = pack_indices(dir);
    assert!(!indices.is_empty());
    let mut pack_oids: Vec<Oid> = Vec::new();
    let mut depth_histogram: Vec<(usize, usize)> = Vec::new();
    let mut deepest: Option<(usize, String)> = None;
    for idx_path in &indices {
        let index = PackIndex::open(idx_path).expect("PackIndex::open");
        let entries = verify_pack(dir, idx_path);
        for entry in &entries {
            assert_eq!(
                index.lookup(entry.oid),
                Some(entry.offset),
                "[{label}] {}: idx offset != git verify-pack offset",
                entry.oid
            );
            assert!(
                set.contains(entry.oid),
                "[{label}] {} is not in PackSet::contains",
                entry.oid
            );
            pack_oids.push(entry.oid);
        }

        let pack_bytes = fs::read(idx_path.with_extension("pack")).expect("read pack");
        let raw = scan_pack(&pack_bytes, &entries);
        assert_eq!(raw.len(), entries.len());

        let mut per_pack_depth: Vec<(usize, usize)> = Vec::new();
        for entry in &entries {
            match per_pack_depth
                .iter_mut()
                .find(|(depth, _)| *depth == entry.depth)
            {
                Some(slot) => slot.1 += 1,
                None => per_pack_depth.push((entry.depth, 1)),
            }
            if entry.depth >= 2 {
                let line = format!(
                    "[{label}] verify-pack depth={} oid={} type={} size={} size_in_pack={} offset={} base={}",
                    entry.depth,
                    entry.oid,
                    entry.kind,
                    entry.size,
                    entry.size_in_pack,
                    entry.offset,
                    entry.base.map(|base| base.to_hex()).unwrap_or_default()
                );
                if deepest
                    .as_ref()
                    .map_or(true, |(best, _)| entry.depth > *best)
                {
                    deepest = Some((entry.depth, line.clone()));
                }
                if entry.depth >= 3 {
                    evidence.push(line);
                }
            }
        }
        per_pack_depth.sort_unstable();
        let summary: Vec<String> = per_pack_depth
            .iter()
            .map(|(depth, count)| format!("depth{depth}={count}"))
            .collect();
        evidence.push(format!(
            "[{label}] {}: {} objects, chain histogram [{}]",
            idx_path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            entries.len(),
            summary.join(", ")
        ));
        for (depth, count) in per_pack_depth {
            match depth_histogram.iter_mut().find(|(d, _)| *d == depth) {
                Some(slot) => slot.1 += count,
                None => depth_histogram.push((depth, count)),
            }
        }
    }
    pack_oids.sort();
    pack_oids.dedup();
    assert_eq!(
        pack_oids, expected_oids,
        "[{label}] git verify-pack oid set differs from `git cat-file --batch-all-objects`"
    );

    // (3) 每个对象逐字节对拍（PackSet::read / Odb::read / 直接 cat-file 三条路径）。
    let odb = Odb::new(&repo);
    for (object, payload) in objects.iter().zip(&truth) {
        let (kind, got_payload) = set
            .read(object.oid)
            .unwrap_or_else(|err| panic!("[{label}] {}: PackSet::read: {err}", object.oid))
            .unwrap_or_else(|| panic!("[{label}] {}: PackSet::read returned None", object.oid));
        assert_eq!(kind.as_str(), object.kind, "[{label}] {}: kind", object.oid);
        assert_eq!(
            got_payload.len() as u64,
            object.size,
            "[{label}] {}: size",
            object.oid
        );
        assert!(
            got_payload == *payload,
            "[{label}] {}: `git cat-file --batch` payload differs",
            object.oid
        );
        assert!(
            got_payload == cat_file_direct(dir, object),
            "[{label}] {}: `git cat-file {} {}` payload differs",
            object.oid,
            object.kind,
            object.oid
        );
        assert_eq!(
            Oid::hash_object(kind.as_str(), &got_payload),
            object.oid,
            "[{label}] {}: payload does not hash back to its oid",
            object.oid
        );
        let (odb_kind, odb_payload) = odb
            .read(object.oid)
            .unwrap_or_else(|err| panic!("[{label}] {}: Odb::read: {err}", object.oid));
        assert_eq!(
            odb_kind.as_str(),
            object.kind,
            "[{label}] {}: Odb kind",
            object.oid
        );
        assert!(
            odb_payload == got_payload,
            "[{label}] {}: Odb payload differs",
            object.oid
        );
    }

    // (4) 不存在的 oid → None / ObjectNotFound，而不是坏数据。
    let absent = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();
    if !expected_oids.contains(&absent) {
        assert!(
            !set.contains(absent),
            "[{label}] phantom oid reported as present"
        );
        assert!(set.read(absent).expect("absent lookup").is_none());
        assert!(
            matches!(odb.read(absent), Err(Error::ObjectNotFound(_))),
            "[{label}] absent oid must be ObjectNotFound"
        );
    }

    depth_histogram.sort_unstable();
    let histogram: Vec<String> = depth_histogram
        .iter()
        .map(|(depth, count)| format!("depth{depth}={count}"))
        .collect();
    evidence.push(format!(
        "[{label}] OK: objects={} packs={} oid-set identical, every object byte-identical to \
         `git cat-file`, idx offsets identical to `git verify-pack`; chain histogram [{}]",
        objects.len(),
        indices.len(),
        histogram.join(", ")
    ));
    if let Some((depth, line)) = deepest {
        evidence.push(format!("[{label}] deepest chain (depth {depth}): {line}"));
    }
    evidence
}

fn repacked_fixture(repack_args: &[&str]) -> tempfile::TempDir {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    fixture(tmp.path());
    git(tmp.path(), repack_args);
    tmp
}

// ---------------------------------------------------------------------------
// 1–3：三种真实 git 配置：集合 + 逐字节 + idx 偏移
// ---------------------------------------------------------------------------

#[test]
fn plain_repack_matches_git_byte_for_byte() {
    let tmp = repacked_fixture(&["repack", "-adf"]);
    let evidence = check_pack_set_against_git(tmp.path(), "repack-adf");
    record("repack-adf", &evidence);
}

#[test]
fn window_depth_repack_matches_git_byte_for_byte() {
    let tmp = repacked_fixture(&["repack", "-adf", "--window=50", "--depth=50"]);
    let evidence = check_pack_set_against_git(tmp.path(), "repack-window50-depth50");
    record("repack-window50-depth50", &evidence);
}

#[test]
fn gc_aggressive_matches_git_byte_for_byte() {
    let tmp = repacked_fixture(&["gc", "--aggressive", "--prune=now"]);
    let evidence = check_pack_set_against_git(tmp.path(), "gc-aggressive");
    record("gc-aggressive", &evidence);
}

// ---------------------------------------------------------------------------
// 4：多级 OFS_DELTA 链（chain length >= 2）
// ---------------------------------------------------------------------------

#[test]
fn multi_level_ofs_delta_chains_decode_byte_for_byte() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    fixture(dir);
    git(dir, &["repack", "-adf"]);

    let idx_path = pack_indices(dir).remove(0);
    let pack_path = idx_path.with_extension("pack");
    let raw_text = verify_pack_raw(dir, &idx_path);
    let entries = parse_verify_pack(&raw_text);
    let pack_bytes = fs::read(&pack_path).unwrap();
    let raw = scan_pack(&pack_bytes, &entries);
    let objects = git_objects(dir);

    let mut evidence = vec![
        format!(
            "[delta-chains] pack={} objects={}",
            pack_path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            entries.len()
        ),
        format!(
            "[delta-chains] git verify-pack -v 汇总: {}",
            raw_text
                .lines()
                .filter(|line| {
                    line.starts_with("non delta:") || line.starts_with("chain length =")
                })
                .collect::<Vec<&str>>()
                .join(" | ")
        ),
    ];

    let index = PackIndex::open(&idx_path).expect("PackIndex::open");
    let pack = PackFile::open(&pack_path).expect("PackFile::open");

    let mut deltas = 0usize;
    let mut max_depth = 0usize;
    for entry in &entries {
        if entry.depth == 0 {
            continue;
        }
        let object = object_of(&objects, entry.oid);
        let (kind, payload) = pack
            .read_at(entry.offset)
            .unwrap_or_else(|err| panic!("{} at {}: {err}", entry.oid, entry.offset));
        assert_eq!(kind.as_str(), object.kind, "{}: kind", entry.oid);
        assert_eq!(payload.len() as u64, object.size, "{}: size", entry.oid);
        assert!(
            payload == cat_file_direct(dir, object),
            "{} (depth {}): delta chain decoded to the wrong bytes",
            entry.oid,
            entry.depth
        );
        assert_eq!(Oid::hash_object(kind.as_str(), &payload), entry.oid);
        assert_eq!(index.lookup(entry.oid), Some(entry.offset));
        deltas += 1;
        max_depth = max_depth.max(entry.depth);
        if entry.depth >= 2 {
            assert_eq!(
                raw_of(&raw, entry.offset).ty,
                6,
                "{}: a multi-level chain must be OFS_DELTA",
                entry.oid
            );
            evidence.push(format!(
                "[delta-chains] depth={} oid={} type={} offset={} base={} payload={}B -> 与 `git cat-file` 逐字节相同",
                entry.depth,
                entry.oid,
                entry.kind,
                entry.offset,
                entry.base.map(|base| base.to_hex()).unwrap_or_default(),
                payload.len()
            ));
        }
    }
    assert!(deltas > 0, "fixture produced no delta objects at all");
    assert!(
        max_depth >= 2,
        "fixture produced no multi-level (chain length >= 2) delta chain: max depth {max_depth}"
    );
    evidence.push(format!(
        "[delta-chains] OK: delta objects={deltas} max chain depth={max_depth}; 每条链与 `git cat-file` 逐字节相同"
    ));
    record("delta-chains", &evidence);
}

// ---------------------------------------------------------------------------
// 5：REF_DELTA（`git pack-objects` 默认不写 delta-base-offset）
// ---------------------------------------------------------------------------

#[test]
fn ref_delta_pack_reads_with_index_and_is_unsupported_without_one() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    fixture(dir);

    // 先把 loose 对象收进 OFS_DELTA pack，再用 pack-objects 造一个 REF_DELTA pack。
    git(dir, &["repack", "-adf"]);
    let revs = git_bytes(dir, &["rev-list", "--objects", "--all"]);
    let pack_bytes = git_stdin(dir, &["pack-objects", "--stdout"], &revs);
    git_stdin(dir, &["index-pack", "--stdin"], &pack_bytes);
    assert_eq!(loose_object_count(dir), 0);

    let mut evidence = Vec::new();
    let mut ref_pack: Option<(PathBuf, PathBuf)> = None;
    for idx_path in pack_indices(dir) {
        let pack_path = idx_path.with_extension("pack");
        let entries = verify_pack(dir, &idx_path);
        let bytes = fs::read(&pack_path).unwrap();
        let raw = scan_pack(&bytes, &entries);
        if raw.iter().any(|object| object.ty == 7) {
            ref_pack = Some((idx_path.clone(), pack_path.clone()));
        }
    }
    let (ref_idx, ref_pack_path) =
        ref_pack.expect("`git pack-objects` produced no REF_DELTA pack to test");

    let index = PackIndex::open(&ref_idx).expect("PackIndex::open");
    let pack = PackFile::open(&ref_pack_path).expect("PackFile::open");
    let entries = verify_pack(dir, &ref_idx);
    let objects = git_objects(dir);
    let mut ref_deltas = 0usize;
    for entry in &entries {
        let object = object_of(&objects, entry.oid);
        let (kind, payload) = pack
            .read_at(entry.offset)
            .unwrap_or_else(|err| panic!("{}: {err}", entry.oid));
        assert_eq!(kind.as_str(), object.kind, "{}", entry.oid);
        assert_eq!(payload.len() as u64, object.size, "{}", entry.oid);
        assert!(
            payload == cat_file_direct(dir, object),
            "{}: REF_DELTA chain decoded to the wrong bytes",
            entry.oid
        );
        assert_eq!(index.lookup(entry.oid), Some(entry.offset));
        if entry.depth >= 1 {
            ref_deltas += 1;
            if entry.depth >= 2 {
                evidence.push(format!(
                    "[ref-delta] depth={} oid={} base={} -> OK",
                    entry.depth,
                    entry.oid,
                    entry.base.map(|base| base.to_hex()).unwrap_or_default()
                ));
            }
        }
    }
    assert!(
        ref_deltas > 0,
        "the REF_DELTA pack contained no delta objects"
    );
    evidence.push(format!(
        "[ref-delta] pack {}: {} objects ({} delta), all byte-identical to `git cat-file`",
        ref_pack_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        entries.len(),
        ref_deltas
    ));

    // 没有 `.idx`：REF_DELTA 必须 Unsupported，非 delta 对象照样可读。
    let bare_dir = tempfile::tempdir().expect("tempdir");
    let bare_path = bare_dir.path().join("pack-without-index.pack");
    fs::copy(&ref_pack_path, &bare_path).unwrap();
    let bare = PackFile::open(&bare_path).expect("PackFile::open without idx");
    let bare_entries = verify_pack(dir, &ref_idx);
    let bare_raw = scan_pack(&fs::read(&bare_path).unwrap(), &bare_entries);

    let ref_object = bare_raw
        .iter()
        .find(|object| object.ty == 7)
        .expect("no REF_DELTA object");
    assert!(
        matches!(bare.read_at(ref_object.offset), Err(Error::Unsupported(_))),
        "REF_DELTA without .idx must be Unsupported"
    );
    assert!(
        matches!(bare.oid_at(ref_object.offset), Err(Error::Unsupported(_))),
        "oid_at without .idx must be Unsupported"
    );

    let stored = bare_raw
        .iter()
        .find(|object| (1..=4).contains(&object.ty))
        .expect("no stored object");
    let stored_entry = bare_entries
        .iter()
        .find(|entry| entry.offset == stored.offset)
        .expect("stored entry");
    let (kind, payload) = bare
        .read_at(stored.offset)
        .expect("stored object must read without an index");
    assert_eq!(kind.as_str(), stored_entry.kind);
    assert_eq!(Oid::hash_object(kind.as_str(), &payload), stored_entry.oid);
    evidence.push(format!(
        "[ref-delta] without .idx: REF_DELTA -> Unsupported; stored object at {} -> {:?} {}B OK",
        stored.offset,
        kind,
        payload.len()
    ));
    record("ref-delta", &evidence);
}

// ---------------------------------------------------------------------------
// 6：多 pack / 同一批 oid 重叠
// ---------------------------------------------------------------------------

#[test]
fn multiple_packs_with_overlapping_objects_agree() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    fixture(dir);
    git(dir, &["repack", "-adf"]);
    let revs = git_bytes(dir, &["rev-list", "--objects", "--all"]);
    let pack_bytes = git_stdin(dir, &["pack-objects", "--stdout"], &revs);
    git_stdin(dir, &["index-pack", "--stdin"], &pack_bytes);

    let indices = pack_indices(dir);
    assert_eq!(indices.len(), 2, "expected two packs, got {indices:?}");
    assert_eq!(loose_object_count(dir), 0);

    let repo = Repo::discover(dir).unwrap();
    let set = PackSet::open(&repo).unwrap();
    assert_eq!(set.indices.len(), 2, "PackSet collapsed the packs");

    let objects = git_objects(dir);
    let expected: Vec<Oid> = objects.iter().map(|object| object.oid).collect();
    assert_eq!(set.iter_oids().unwrap(), expected);

    let first = PackFile::open(&indices[0].with_extension("pack")).unwrap();
    let second = PackFile::open(&indices[1].with_extension("pack")).unwrap();
    let first_idx = PackIndex::open(&indices[0]).unwrap();
    let second_idx = PackIndex::open(&indices[1]).unwrap();
    for object in &objects {
        let a = first
            .read_at(first_idx.lookup(object.oid).expect("in pack A"))
            .unwrap_or_else(|err| panic!("{} (pack A): {err}", object.oid));
        let b = second
            .read_at(second_idx.lookup(object.oid).expect("in pack B"))
            .unwrap_or_else(|err| panic!("{} (pack B): {err}", object.oid));
        assert_eq!(a.0, b.0, "{}: kind differs between packs", object.oid);
        assert!(a.1 == b.1, "{}: payload differs between packs", object.oid);
        assert!(
            a.1 == cat_file_direct(dir, object),
            "{}: differs from git",
            object.oid
        );
        let via_set = set.read(object.oid).unwrap().expect("PackSet hit");
        assert_eq!(via_set.0, a.0);
        assert!(via_set.1 == a.1);
    }
    record(
        "multi-pack",
        &[format!(
            "[multi-pack] OK: {} indices, {} oids present in both packs, all identical to each \
             other and to `git cat-file`; PackSet::iter_oids 与 git 集合相同",
            indices.len(),
            objects.len()
        )],
    );
}

// ---------------------------------------------------------------------------
// 7：结构性反例（必须 Err(Corrupt)，不得静默跳过 / 返回部分数据）
// ---------------------------------------------------------------------------

#[test]
fn corrupt_index_and_pack_files_are_rejected() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    fixture(dir);
    git(dir, &["repack", "-adf"]);

    let good_idx_path = pack_indices(dir).remove(0);
    let good_pack_path = good_idx_path.with_extension("pack");
    let good_idx = fs::read(&good_idx_path).unwrap();
    let good_pack = fs::read(&good_pack_path).unwrap();
    let scratch = tempfile::tempdir().expect("tempdir");
    let out = scratch.path();
    let mut evidence = Vec::new();

    let write = |name: &str, bytes: &[u8]| -> PathBuf {
        let path = out.join(name);
        fs::write(&path, bytes).unwrap();
        path
    };

    // (a) idx 截断。
    let path = write("truncated.idx", &good_idx[..good_idx.len() - 3]);
    assert!(
        matches!(PackIndex::open(&path), Err(Error::Corrupt { .. })),
        "truncated .idx must be Corrupt"
    );

    // (b) idx magic 被改。
    let mut bad = good_idx.clone();
    bad[0] = b'X';
    let path = write("bad-magic.idx", &bad);
    assert!(matches!(PackIndex::open(&path), Err(Error::Corrupt { .. })));

    // (c) idx version != 2（校验和自洽，只有结构检查能抓到）。
    let mut bad = good_idx.clone();
    write_u32be(&mut bad, 4, 3);
    resign_index(&mut bad);
    let path = write("bad-version.idx", &bad);
    assert!(
        matches!(PackIndex::open(&path), Err(Error::Corrupt { .. })),
        "version 3 .idx must be Corrupt"
    );

    // (d) idx 自身校验和不对。
    let mut bad = good_idx.clone();
    let last = bad.len() - 1;
    bad[last] ^= 0xff;
    let path = write("bad-checksum.idx", &bad);
    assert!(matches!(PackIndex::open(&path), Err(Error::Corrupt { .. })));
    evidence.push(
        "[corrupt] .idx: truncated / magic / version=3 / trailing checksum -> 全部 Corrupt"
            .to_string(),
    );

    // (e) fanout 非单调。
    let mut bad = good_idx.clone();
    write_u32be(&mut bad, IDX_HEADER_LEN + 250 * 4, 0xffff_ffff);
    resign_index(&mut bad);
    let path = write("bad-fanout.idx", &bad);
    assert!(
        matches!(PackIndex::open(&path), Err(Error::Corrupt { .. })),
        "non-monotonic fanout must be Corrupt"
    );
    evidence.push("[corrupt] .idx fanout 非单调 -> Corrupt".to_string());

    // (f) pack 截断 / magic / version / 校验和。
    let path = write("truncated.pack", &good_pack[..good_pack.len() / 2]);
    assert!(matches!(PackFile::open(&path), Err(Error::Corrupt { .. })));
    let mut bad = good_pack.clone();
    bad[0] = b'X';
    let path = write("bad-magic.pack", &bad);
    assert!(matches!(PackFile::open(&path), Err(Error::Corrupt { .. })));
    let mut bad = good_pack.clone();
    write_u32be(&mut bad, 4, 9);
    resign_pack(&mut bad);
    let path = write("bad-version.pack", &bad);
    assert!(
        matches!(PackFile::open(&path), Err(Error::Corrupt { .. })),
        "pack version 9 must be Corrupt"
    );
    let mut bad = good_pack.clone();
    let last = bad.len() - 1;
    bad[last] ^= 0xff;
    let path = write("bad-checksum.pack", &bad);
    assert!(matches!(PackFile::open(&path), Err(Error::Corrupt { .. })));
    evidence.push(
        "[corrupt] .pack: truncated / magic / version=9 / trailing checksum -> 全部 Corrupt"
            .to_string(),
    );

    // (g) 保留类型 5：open 成功，read_at 必须 Corrupt。
    let entries = verify_pack(dir, &good_idx_path);
    let raw = scan_pack(&good_pack, &entries);
    let stored = raw
        .iter()
        .find(|object| (1..=4).contains(&object.ty))
        .expect("no stored object");
    let mut bad = good_pack.clone();
    let header_pos = usize::try_from(stored.offset).unwrap();
    bad[header_pos] = (bad[header_pos] & 0x8f) | (5 << 4);
    resign_pack(&mut bad);
    let path = write("reserved-type.pack", &bad);
    let pack = PackFile::open(&path).expect("pack shell is still valid");
    let detail = corrupt_detail(pack.read_at(stored.offset).unwrap_err());
    assert!(
        detail.contains("reserved"),
        "expected the reserved-type Corrupt, got: {detail}"
    );
    evidence.push(format!("[corrupt] 对象类型 5 -> Corrupt ({detail})"));

    // (h) 真实 delta 负载被换成「copy 越界」：必须 Corrupt，而且落在 copy 越界那条分支。
    let delta_entry = entries
        .iter()
        .find(|entry| entry.depth >= 1 && entry.base.is_some())
        .expect("no delta with a base");
    let delta_raw = raw_of(&raw, delta_entry.offset);
    let base_size = object_of(&git_objects(dir), delta_entry.base.expect("base oid")).size;
    let mut evil = varint(base_size);
    evil.extend_from_slice(&varint(base_size));
    // 0xff = copy + 4 个 offset 字节 + 3 个 size 字节（size 全 0 → 默认 0x10000）。
    evil.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x7f, 0x00, 0x00, 0x00]);
    let mut bad = Vec::new();
    bad.extend_from_slice(&good_pack[..usize::try_from(delta_entry.offset).unwrap()]);
    bad.extend_from_slice(&pack_object_header(6, evil.len() as u64));
    // 原样保留 OFS_DELTA 的负偏移段。
    let base_varint_start = delta_raw.data_pos - delta_raw.base_varint_len;
    bad.extend_from_slice(&good_pack[base_varint_start..delta_raw.data_pos]);
    bad.extend_from_slice(&minigit::zlib::deflate(&evil).expect("deflate"));
    bad.extend_from_slice(&good_pack[delta_raw.data_pos + delta_raw.compressed_len..]);
    resign_pack(&mut bad);
    let path = write("delta-copy-out-of-bounds.pack", &bad);
    let pack = PackFile::open(&path).expect("pack shell is still valid");
    let detail = corrupt_detail(pack.read_at(delta_entry.offset).unwrap_err());
    assert!(
        detail.contains("outside the base"),
        "expected a copy-range Corrupt, got: {detail}"
    );
    evidence.push(format!(
        "[corrupt] 改写真实 delta：copy 超出 base -> Corrupt ({detail})"
    ));

    // (j) 大 offset 表：把首个条目的偏移换成「大 offset 索引 0」，并在尾部插入 8 字节表，
    //     值为真实的 12（< 2^31）。git 只在 offset >= 2^31 时才走大 offset 表，所以这是
    //     畸形索引；实现选择严格拒绝（idx.rs 的 BIG_OFFSET_MIN）。这条覆盖了「真实的
    //     git idx + 大 offset 分支」的解析路径（>2GB 的真 pack 本机造不出来，见 result）。
    let mut big = good_idx.clone();
    let (n_entries, _oid_table, offset_table) = idx_table_positions(&good_idx);
    assert!(n_entries > 0);
    let first_offset = u32::from_be_bytes([
        big[offset_table],
        big[offset_table + 1],
        big[offset_table + 2],
        big[offset_table + 3],
    ]);
    assert!(
        first_offset < 0x8000_0000,
        "fixture pack offsets are unexpectedly huge"
    );
    let trailer_start = big.len() - 40;
    write_u32be(&mut big, offset_table, 0x8000_0000); // 大 offset 表的第 0 项
    let mut rebuilt = Vec::with_capacity(big.len() + 8);
    rebuilt.extend_from_slice(&big[..trailer_start]);
    rebuilt.extend_from_slice(&u64::from(first_offset).to_be_bytes());
    rebuilt.extend_from_slice(&big[trailer_start..]);
    resign_index(&mut rebuilt);
    let path = write("small-big-offset.idx", &rebuilt);
    let detail = corrupt_detail(PackIndex::open(&path).unwrap_err());
    assert!(
        detail.contains("2^31"),
        "a 64-bit offset table entry below 2^31 must be rejected, got: {detail}"
    );
    evidence.push(format!(
        "[corrupt] 真实 idx + 大 offset 表指向 < 2^31 的值 -> Corrupt ({detail})"
    ));

    // (i) 坏 idx 与好 pack 并存：PackSet::open 必须整体 Corrupt（不许静默跳过）。
    let injected = dir
        .join(".git/objects/pack")
        .join("pack-0000000000000000000000000000000000000000.idx");
    fs::write(&injected, b"definitely not a pack index").unwrap();
    let repo = Repo::discover(dir).unwrap();
    assert!(
        matches!(PackSet::open(&repo), Err(Error::Corrupt { .. })),
        "a corrupt .idx beside a healthy pack must be Corrupt, not silently skipped"
    );
    fs::remove_file(&injected).unwrap();
    evidence.push(
        "[corrupt] 好 pack 旁边的垃圾 .idx -> PackSet::open Corrupt（未静默跳过）".to_string(),
    );

    record("corrupt", &evidence);
}

// ---------------------------------------------------------------------------
// 8：idx 与 pack 不一致（oid → 别人的偏移）必须是 Corrupt，不能返回错数据
// ---------------------------------------------------------------------------

#[test]
fn index_offset_mismatch_is_corrupt_not_wrong_data() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    fixture(dir);
    git(dir, &["repack", "-adf"]);

    let good_idx_path = pack_indices(dir).remove(0);
    let good_pack_path = good_idx_path.with_extension("pack");
    let good_idx = fs::read(&good_idx_path).unwrap();
    let entries = verify_pack(dir, &good_idx_path);
    // 真值必须在注入坏 idx **之前**取：实测一旦仓库里多了一个映射错误的 idx，
    // 连 `git cat-file --batch-all-objects` 自己都会按那份坏映射报类型（git 也信 idx）。
    let objects = git_objects(dir);
    let truth = cat_file_batch(dir, &objects);

    // 选两个偏移 < 2^31 的条目（避开大 offset 表），交换它们在 offset 表里的值：
    // oid 表仍升序、fanout 仍自洽，只有「oid → 偏移」的映射错了。
    let (n, oid_table, offset_table) = idx_table_positions(&good_idx);
    assert_eq!(n, entries.len());
    let mut ordered: Vec<&PackEntry> = entries.iter().collect();
    ordered.sort_by_key(|entry| entry.offset);
    assert!(ordered.len() >= 3);
    let (a, b) = (ordered[1], ordered[2]);
    assert!(a.offset < 0x8000_0000 && b.offset < 0x8000_0000);

    let position_of = |oid: Oid| -> usize {
        (0..n)
            .find(|i| good_idx[oid_table + i * 20..oid_table + i * 20 + 20] == oid.as_bytes()[..])
            .expect("oid is in the index table")
    };
    let mut bad = good_idx.clone();
    let (ia, ib) = (position_of(a.oid), position_of(b.oid));
    write_u32be(&mut bad, offset_table + ia * 4, b.offset as u32);
    write_u32be(&mut bad, offset_table + ib * 4, a.offset as u32);
    resign_index(&mut bad);

    // 注入一个排在最前的 idx/pack 对（pack 字节与真 pack 相同，只有 idx 映射错）。
    let pack_dir = dir.join(".git/objects/pack");
    let fake_idx = pack_dir.join("pack-0000000000000000000000000000000000000000.idx");
    let fake_pack = fake_idx.with_extension("pack");
    fs::write(&fake_idx, &bad).unwrap();
    fs::copy(&good_pack_path, &fake_pack).unwrap();

    let repo = Repo::discover(dir).unwrap();
    let set = PackSet::open(&repo).expect("the mutated idx is structurally valid");
    let err = set
        .read(a.oid)
        .expect_err("the oid -> offset mapping is wrong: read must not succeed");
    let detail = corrupt_detail(err);
    assert!(
        detail.contains("hash") || detail.contains("maps"),
        "expected a hash-mismatch Corrupt, got: {detail}"
    );

    // 偏移本身是好的：按 a 的真实偏移读，内容仍然正确（说明变异只打在映射上）。
    let pack = PackFile::open(&fake_pack).unwrap();
    let (kind, payload) = pack.read_at(a.offset).unwrap();
    let index = objects
        .iter()
        .position(|object| object.oid == a.oid)
        .unwrap();
    assert_eq!(kind.as_str(), objects[index].kind);
    assert!(
        payload == truth[index],
        "reading at the real offset{} must still yield git's bytes",
        ""
    );

    fs::remove_file(&fake_idx).unwrap();
    fs::remove_file(&fake_pack).unwrap();
    check_pack_set_against_git(dir, "after-idx-mismatch-restore");

    record(
        "idx-mismatch",
        &[
            format!(
                "[idx-mismatch] 把 {} 的偏移改成 {}（属于 {}）后 read -> Corrupt: {}",
                a.oid, b.offset, b.oid, detail
            ),
            "[idx-mismatch] OK: 重算 oid 校验拦住了错数据；按真实偏移仍能读出正确内容".to_string(),
        ],
    );
}

// ---------------------------------------------------------------------------
// 9：逐字节翻转 pack（重算 sha1）—— 绝不返回错数据
// ---------------------------------------------------------------------------

#[test]
fn pack_byte_flips_never_yield_wrong_object_data() {
    require_git();
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    fixture(dir);
    git(dir, &["repack", "-adf"]);

    let idx_path = pack_indices(dir).remove(0);
    let pack_path = idx_path.with_extension("pack");
    let good = fs::read(&pack_path).unwrap();
    let entries = verify_pack(dir, &idx_path);
    let raw = scan_pack(&good, &entries);
    let objects = git_objects(dir);
    let scratch = tempfile::tempdir().expect("tempdir");

    let mut inspected = 0usize;
    let mut head_corruptions = 0usize;
    let mut mid_ok_but_identical = 0usize;
    for object in raw.iter().take(12) {
        inspected += 1;
        let entry = entries
            .iter()
            .find(|entry| entry.offset == object.offset)
            .expect("verify-pack entry");
        let truth = object_of(&objects, entry.oid);

        // (i) zlib 流首字节被破坏 → 解码必然失败。
        let mut broken = good.clone();
        broken[object.data_pos] ^= 0xff;
        resign_pack(&mut broken);
        let path = scratch
            .path()
            .join(format!("zlib-head-{}.pack", object.offset));
        fs::write(&path, &broken).unwrap();
        let pack = PackFile::open(&path).expect("pack shell ok");
        match pack.read_at(object.offset) {
            Err(Error::Corrupt { .. }) => head_corruptions += 1,
            other => panic!(
                "object at {}: a broken zlib header must be Corrupt, got {other:?}",
                object.offset
            ),
        }

        // (ii) zlib 流中段被破坏 → 只允许 Err(Corrupt)，或（理论上）解出的字节与 git 完全一致。
        let mut broken = good.clone();
        broken[object.data_pos + object.compressed_len / 2] ^= 0xff;
        resign_pack(&mut broken);
        let path = scratch
            .path()
            .join(format!("zlib-mid-{}.pack", object.offset));
        fs::write(&path, &broken).unwrap();
        let pack = PackFile::open(&path).expect("pack shell ok");
        match pack.read_at(object.offset) {
            Err(Error::Corrupt { .. }) => {}
            Ok((kind, payload)) => {
                assert_eq!(
                    kind.as_str(),
                    truth.kind,
                    "{}: kind after mid-stream flip",
                    entry.oid
                );
                assert!(
                    payload == cat_file_direct(dir, truth),
                    "{}: mid-stream flip returned bytes that are NOT git's: silent wrong data",
                    entry.oid
                );
                mid_ok_but_identical += 1;
            }
            Err(other) => panic!("{}: expected Corrupt, got {other:?}", entry.oid),
        }

        // (iii) PackSet::read 也必须 Corrupt（不能退化成 ObjectNotFound）。
        let mut broken = good.clone();
        broken[object.data_pos] ^= 0xff;
        resign_pack(&mut broken);
        force_write(&pack_path, &broken);
        let repo = Repo::discover(dir).unwrap();
        let set = PackSet::open(&repo).unwrap();
        let err = set
            .read(entry.oid)
            .expect_err("a corrupt object must not read as Ok");
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "{}: expected Corrupt from PackSet::read, got {err:?}",
            entry.oid
        );
        force_write(&pack_path, &good);
    }

    assert!(inspected > 0, "no object was inspected");
    check_pack_set_against_git(dir, "after-byte-flip-restore");
    record(
        "byte-flips",
        &[format!(
            "[byte-flips] 检查 {inspected} 个对象：zlib 首字节破坏 -> Corrupt x{head_corruptions}；\
             中段破坏 -> 未出现任何「非 git 字节」（其中 {mid_ok_but_identical} 次仍与 git 逐字节相同）；\
             PackSet::read -> Corrupt（不是 ObjectNotFound）"
        )],
    );
}

// ---------------------------------------------------------------------------
// 10：delta 模块直接调用（公共 API）的反例
// ---------------------------------------------------------------------------

#[test]
fn delta_module_rejects_malformed_instruction_streams() {
    use minigit::odb::pack::delta::{apply_delta, parse_delta_header};

    let base = b"0123456789";

    // 合法：copy(base[2..4]) + insert("hi") → 4 字节。
    let ok = {
        let mut delta = varint(base.len() as u64);
        delta.extend_from_slice(&varint(4));
        // 0x91 = copy + 1 个 offset 字节(0x02) + 1 个 size 字节(0x02)；0x02 = insert 2 字节。
        delta.extend_from_slice(&[0x91, 0x02, 0x02, 0x02, b'h', b'i']);
        delta
    };
    let header = parse_delta_header(&ok).expect("header");
    assert_eq!((header.base_size, header.target_size), (10, 4));
    assert!(apply_delta(base, &ok).unwrap() == b"23hi");

    // 头声明 base_size 与 base 不符。
    let mut delta = varint(99);
    delta.extend_from_slice(&varint(1));
    delta.extend_from_slice(&[0x01, b'x']);
    assert!(matches!(
        apply_delta(base, &delta),
        Err(Error::Corrupt { .. })
    ));

    // 保留 opcode 0x00。
    let mut delta = varint(10);
    delta.extend_from_slice(&varint(1));
    delta.push(0x00);
    assert!(matches!(
        apply_delta(base, &delta),
        Err(Error::Corrupt { .. })
    ));

    // copy 越界（offset 10 + size 1 > 10）。
    let mut delta = varint(10);
    delta.extend_from_slice(&varint(1));
    delta.extend_from_slice(&[0x91, 0x0a, 0x01]);
    let detail = corrupt_detail(apply_delta(base, &delta).unwrap_err());
    assert!(detail.contains("outside the base"), "got: {detail}");

    // 结果长度 < target_size。
    let mut delta = varint(10);
    delta.extend_from_slice(&varint(5));
    delta.extend_from_slice(&[0x01, b'x']);
    assert!(matches!(
        apply_delta(base, &delta),
        Err(Error::Corrupt { .. })
    ));

    // 结果长度 > target_size。
    let mut delta = varint(10);
    delta.extend_from_slice(&varint(1));
    delta.extend_from_slice(&[0x02, b'x', b'y']);
    assert!(matches!(
        apply_delta(base, &delta),
        Err(Error::Corrupt { .. })
    ));

    // insert 长度超出 delta 剩余字节。
    let mut delta = varint(10);
    delta.extend_from_slice(&varint(4));
    delta.extend_from_slice(&[0x7f, b'x']);
    assert!(matches!(
        apply_delta(base, &delta),
        Err(Error::Corrupt { .. })
    ));

    // 截断的头。
    assert!(matches!(
        parse_delta_header(&[0x80]),
        Err(Error::Corrupt { .. })
    ));

    record(
        "delta-api",
        &["[delta-api] OK: base-size 不符 / opcode 0x00 / copy 越界 / target-size 两个方向不符 / \
           insert 越界 / 头截断 -> 全部 Corrupt（无 panic）"
            .to_string()],
    );
}
