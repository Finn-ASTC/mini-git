//! V13 —— 独立验证 T13：`file://` 传输 + `clone` / `fetch` / `push` / `pull`。
//!
//! 判定原则（本轮验证者 omp，作者 codex）：
//!
//! * **真值只有真实 `git` 进程**：oid 集合来自 `git show-ref` / `git rev-parse`，
//!   字节来自 `git upload-pack --advertise-refs` / `git receive-pack`，
//!   pack 的合法性来自 `git index-pack` / `git unpack-objects`，仓库完好性来自 `git fsck`。
//! * **零硬编码真值**：本文件不写死任何 oid、包内容或「应该是这样」的常量；
//!   连「mg clone 的结果对不对」也是与**同源的真实 `git clone`** 逐项对拍。
//! * **pkt-line 是字节级检查的**：`build_fetch_request` / `build_push_update` 的期望字节由
//!   本文件的 `frame()` 手工分帧拼出（不借助被测的编码器），并与真实 git 的服务端互相喂字节。
//! * **环境缺失一律硬失败**（`git` 不在 PATH、`mg` 二进制没构建 → panic），
//!   不允许「skip 即通过」（W3/V9 的假绿）。
//!
//! 夹具（`source_repo`）的完整状态：2 个提交、子目录 `sub/b.txt`、执行位文件 `run.sh`(100755)、
//! 分支 `feature`、附注 tag `v1`、轻量 tag `light`；提交者身份与时间由 `git_cmd()` 固定，
//! 因此同一夹具在任何机器上产出同一批 oid。

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use minigit::oid::Oid;
use minigit::refs::RefStore;
use minigit::repo::Repo;
use minigit::transport::local;
use minigit::transport::negotiate::{
    build_fetch_request, build_push_update, parse_advertisement, parse_report_status, FetchRequest,
    PushCommand,
};
use minigit::transport::pktline::{read_pkt, write_flush, write_pkt, Pkt};

/// `cargo` 为集成测试注入的被测二进制路径（`[[bin]] name = "mg"`）。
const MG: &str = env!("CARGO_BIN_EXE_mg");

// ---------------------------------------------------------------- harness

fn git_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "A U Thor")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_NAME", "A U Thor")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_AUTHOR_DATE", "1700000000 +0000")
        .env("GIT_COMMITTER_DATE", "1700000000 +0000")
        .env("LC_ALL", "C");
    cmd
}

fn out_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn git_raw(dir: &Path, args: &[&str]) -> Output {
    git_cmd(dir)
        .args(args)
        .output()
        .expect("this verification requires a real `git` on PATH; missing git must fail, not skip")
}

fn git_ok(dir: &Path, args: &[&str]) -> Output {
    let out = git_raw(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} failed ({}): {}",
        out.status,
        out_text(&out.stderr)
    );
    out
}

fn git_text(dir: &Path, args: &[&str]) -> String {
    let out = git_ok(dir, args);
    String::from_utf8(out.stdout)
        .expect("git stdout is UTF-8")
        .trim_end_matches('\n')
        .to_string()
}

fn git_lines(dir: &Path, args: &[&str]) -> Vec<String> {
    git_text(dir, args).lines().map(str::to_string).collect()
}

fn mg_raw(dir: &Path, args: &[&str]) -> Output {
    Command::new(MG)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("the mg binary must be built by `cargo test`; a missing binary must fail, not skip")
}

fn mg_ok(dir: &Path, args: &[&str]) -> Output {
    let out = mg_raw(dir, args);
    assert!(
        out.status.success(),
        "mg {args:?} failed ({}): {}{}",
        out.status,
        out_text(&out.stdout),
        out_text(&out.stderr)
    );
    out
}

/// 期望失败的 `mg`：返回 stderr，并硬断言「非零退出」「没有 panic」。
fn mg_fail(dir: &Path, args: &[&str]) -> String {
    let out = mg_raw(dir, args);
    assert!(
        !out.status.success(),
        "mg {args:?} unexpectedly succeeded: {}",
        out_text(&out.stdout)
    );
    let stderr = out_text(&out.stderr);
    assert!(
        !stderr.contains("panicked"),
        "mg {args:?} panicked: {stderr}"
    );
    stderr
}

/// `FETCH_HEAD` 里出现的 oid 集合（每行第一个字段），排序后返回。
fn fetch_head_oids(repo_dir: &Path) -> Vec<String> {
    let text = fs::read_to_string(repo_dir.join(".git/FETCH_HEAD")).expect("FETCH_HEAD");
    let mut oids: Vec<String> = text
        .lines()
        .filter_map(|line| line.split('\t').next())
        .map(str::to_string)
        .collect();
    oids.sort();
    oids.dedup();
    oids
}

fn write_file(root: &Path, rel: &str, contents: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("creating a fixture directory");
    }
    fs::write(path, contents).expect("writing a fixture file");
}

fn commit_all(dir: &Path, message: &str) {
    git_ok(dir, &["add", "-A"]);
    git_ok(dir, &["commit", "-q", "-m", message]);
}

fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// 真实 git 建的标准夹具；见文件头的完整状态说明。
fn source_repo(root: &Path) -> PathBuf {
    let src = root.join("src");
    fs::create_dir_all(&src).expect("creating the fixture");
    git_ok(&src, &["init", "-q", "-b", "main", "."]);
    write_file(&src, "a.txt", b"one\n");
    write_file(&src, "sub/b.txt", b"beta\n");
    write_file(&src, "run.sh", b"#!/bin/sh\necho hi\n");
    fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755))
        .expect("setting the exec bit");
    commit_all(&src, "one");
    git_ok(&src, &["branch", "feature"]);
    write_file(&src, "a.txt", b"one\ntwo\n");
    commit_all(&src, "two");
    git_ok(&src, &["tag", "-a", "v1", "-m", "tag message"]);
    git_ok(&src, &["tag", "light"]);
    src
}

fn bare_repo(root: &Path, name: &str) -> PathBuf {
    let bare = root.join(name);
    git_ok(
        root,
        &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    bare
}

/// 手工 pkt-line 分帧（`<4 位小写十六进制长度><payload>`）—— 刻意不用被测的编码器。
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", payload.len() + 4).into_bytes();
    out.extend_from_slice(payload);
    out
}

fn advertise_bytes(git_dir: &Path) -> Vec<u8> {
    git_ok(
        git_dir,
        &[
            "-c",
            "protocol.version=0",
            "upload-pack",
            "--advertise-refs",
            ".",
        ],
    )
    .stdout
}

/// 逐帧读一个 upload-pack 响应：返回 `(pack 字节, 是否出现过进度通道)`。
fn collect_side_band(response: &[u8]) -> (Vec<u8>, bool, bool) {
    let mut cursor = Cursor::new(response);
    let mut pack = Vec::new();
    let mut progress = false;
    let mut saw_ack = false;
    loop {
        match read_pkt(&mut cursor).expect("the response must be valid pkt-line") {
            Pkt::Data(data) => match data.first().copied() {
                Some(1) => {
                    assert!(saw_ack, "pack data must come after NAK/ACK");
                    pack.extend_from_slice(&data[1..]);
                }
                Some(2) => progress = true,
                Some(3) => panic!("git reported a fatal error: {}", out_text(&data[1..])),
                _ => {
                    let line = out_text(&data);
                    assert!(
                        line.starts_with("NAK") || line.starts_with("ACK"),
                        "first response frame must be NAK/ACK, got {line:?}"
                    );
                    saw_ack = true;
                }
            },
            Pkt::Flush => break,
            other => panic!("unexpected frame in an upload-pack response: {other:?}"),
        }
    }
    (pack, progress, saw_ack)
}

/// 把请求喂给真实 `git receive-pack --stateless-rpc`，返回它的原始输出。
fn run_receive_pack(bare: &Path, request: &[u8]) -> Output {
    let mut child = git_cmd(bare)
        .args([
            "-c",
            "protocol.version=0",
            "receive-pack",
            "--stateless-rpc",
            ".",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning git receive-pack");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(request)
        .expect("writing the push request");
    child.wait_with_output().expect("receive-pack output")
}

/// 把请求喂给真实 `git upload-pack --stateless-rpc`，返回它的原始输出。
fn run_upload_pack(git_dir: &Path, request: &[u8]) -> Output {
    let mut child = git_cmd(git_dir)
        .args([
            "-c",
            "protocol.version=0",
            "upload-pack",
            "--stateless-rpc",
            ".",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning git upload-pack");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(request)
        .expect("writing the fetch request");
    child.wait_with_output().expect("upload-pack output")
}

/// 仓库里**每个文件**的路径 → 内容摘要。用来断言「远端一个字节都没变」。
fn repo_digest(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, rel: &Path, out: &mut BTreeMap<String, String>) {
        let dir = root.join(rel);
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap_or_else(|err| panic!("reading {}: {err}", dir.display()))
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path
                .strip_prefix(root)
                .expect("path under root")
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                walk(root, Path::new(&name), out);
            } else {
                let bytes = fs::read(&path).unwrap_or_else(|err| panic!("reading {name}: {err}"));
                let oid = Oid::hash_object("blob", &bytes).to_hex();
                out.insert(name, oid);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, Path::new(""), &mut out);
    out
}

// ---------------------------------------------------------------- A. 协议：字节级

/// `git upload-pack --advertise-refs` 的**原始字节**必须能被 `parse_advertisement` 解析成
/// 「与 `git show-ref` / `git rev-parse` 完全相等」的引用集合，并且逐帧重编码后逐字节相同。
#[test]
fn advertisement_from_real_git_matches_show_ref_and_reframes_byte_exactly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = source_repo(dir.path());
    let git_dir = src.join(".git");
    let raw = advertise_bytes(&git_dir);

    // 任务书提示「本地直跑 upload-pack --advertise-refs 的输出开头有 service 行」——
    // 实测为假：`# service=git-upload-pack` 只在 smart HTTP 的 `info/refs` 主体里出现。
    assert!(
        !raw.windows(9).any(|window| window == b"# service"),
        "a locally-run upload-pack must not emit the HTTP service line"
    );

    let adv = parse_advertisement(&raw).expect("real advertisement must parse");

    // 引用集合 == `git show-ref`（排除 HEAD 与附注 tag 的 `^{}` 剥离行）。
    let mut want: BTreeMap<String, String> = BTreeMap::new();
    for line in git_lines(&src, &["show-ref"]) {
        let (oid, name) = line.split_once(' ').expect("`git show-ref` line shape");
        want.insert(name.to_string(), oid.to_string());
    }
    let mut got: BTreeMap<String, String> = BTreeMap::new();
    for (name, oid) in &adv.refs {
        if name == "HEAD" || name.ends_with("^{}") {
            continue;
        }
        got.insert(name.clone(), oid.to_hex());
    }
    assert_eq!(got, want, "parsed refs must equal `git show-ref` exactly");

    // HEAD 行与 symref 都来自真实 git。
    let head = git_text(&src, &["rev-parse", "HEAD"]);
    assert_eq!(
        adv.get("HEAD").map(|oid| oid.to_hex()).as_deref(),
        Some(head.as_str())
    );
    let symref = git_text(&src, &["symbolic-ref", "HEAD"]);
    assert_eq!(adv.head_symref(), Some(symref.as_str()));
    assert!(
        adv.has_capability("side-band-64k"),
        "{:?}",
        adv.capabilities
    );
    assert!(!adv.is_empty_repository());

    // 附注 tag 的剥离行 == `git rev-parse v1^{}`，且 `refs/tags/light` 指向提交本身。
    let peeled = git_text(&src, &["rev-parse", "v1^{}"]);
    assert_eq!(
        adv.get("refs/tags/v1^{}")
            .map(|oid| oid.to_hex())
            .as_deref(),
        Some(peeled.as_str())
    );
    assert_eq!(
        adv.get("refs/tags/light")
            .map(|oid| oid.to_hex())
            .as_deref(),
        Some(head.as_str())
    );
    assert_ne!(
        adv.get("refs/tags/v1").map(|oid| oid.to_hex()),
        Some(peeled),
        "an annotated tag advertises the tag object, not the commit"
    );

    // 逐帧重编码 == 真实 git 的字节（含首行 NUL 后的 capability 与结尾 flush）。
    let mut rebuilt = Vec::new();
    for (idx, (name, oid)) in adv.refs.iter().enumerate() {
        let mut line = format!("{} {name}", oid.to_hex());
        if idx == 0 {
            line.push('\0');
            line.push_str(&adv.capabilities.join(" "));
        }
        line.push('\n');
        write_pkt(&mut rebuilt, line.as_bytes()).expect("writing to a Vec");
    }
    write_flush(&mut rebuilt).expect("writing to a Vec");
    assert_eq!(
        rebuilt, raw,
        "re-encoding the parsed advertisement must reproduce git's bytes"
    );

    // 帧结构自证：首帧是 `<oid> HEAD\0<caps>`，流以 flush 收尾。
    let mut cursor = Cursor::new(raw.as_slice());
    let first = match read_pkt(&mut cursor).expect("valid pkt-line") {
        Pkt::Data(data) => data,
        other => panic!("first frame must be data, got {other:?}"),
    };
    let mut expect_first = format!("{head} HEAD").into_bytes();
    expect_first.push(0);
    assert!(
        first.starts_with(&expect_first),
        "first frame: {:?}",
        out_text(&first[..60.min(first.len())])
    );
    assert_eq!(&raw[raw.len() - 4..], b"0000");
}

/// `build_fetch_request` 的字节必须能被真实 `git upload-pack --stateless-rpc` 接受，
/// 且它回的 side-band 通道 1 里的 pack 能被真实 git 解出来（对象与源仓库逐字节相同）。
#[test]
fn our_fetch_request_round_trips_through_real_git_upload_pack() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = source_repo(dir.path());
    let git_dir = src.join(".git");
    let adv = parse_advertisement(&advertise_bytes(&git_dir)).expect("advertisement");

    let head = adv.get("HEAD").expect("HEAD advertised");
    let tag = adv
        .get("refs/tags/v1")
        .expect("the annotated tag is advertised");
    let request = build_fetch_request(
        &FetchRequest {
            wants: vec![head, tag],
            haves: Vec::new(),
            done: true,
        },
        &["side-band-64k", "no-progress"],
    );

    // 字节级：期望流由本文件手工分帧拼出（不经被测编码器）。
    let mut expect = Vec::new();
    expect.extend_from_slice(&frame(
        format!("want {} side-band-64k no-progress\n", head.to_hex()).as_bytes(),
    ));
    expect.extend_from_slice(&frame(format!("want {}\n", tag.to_hex()).as_bytes()));
    expect.extend_from_slice(b"0000");
    expect.extend_from_slice(&frame(b"done\n"));
    assert_eq!(request, expect, "fetch request bytes must be exactly v0");

    let out = run_upload_pack(&git_dir, &request);
    assert!(
        out.status.success(),
        "git upload-pack rejected our request: {}",
        out_text(&out.stderr)
    );

    let (pack, progress, saw_ack) = collect_side_band(&out.stdout);
    assert!(saw_ack, "the response must start with NAK/ACK");
    assert!(!progress, "we asked for `no-progress`");
    assert!(pack.starts_with(b"PACK"), "band 1 must carry a pack");

    // 真实 git 验证这个 pack：`index-pack --stdin --strict` 自己算 sha1 并写 idx。
    let fresh = dir.path().join("fresh");
    fs::create_dir_all(&fresh).expect("fresh dir");
    git_ok(&fresh, &["init", "-q", "-b", "main", "."]);
    let mut child = git_cmd(&fresh)
        .args(["index-pack", "--stdin", "--strict"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning git index-pack");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&pack)
        .expect("pack");
    let indexed = child.wait_with_output().expect("index-pack output");
    assert!(
        indexed.status.success(),
        "git index-pack rejected the pack we received: {}",
        out_text(&indexed.stderr)
    );
    let stdout = out_text(&indexed.stdout);
    let sha = stdout.split_whitespace().last().unwrap_or_default();
    assert_eq!(sha.len(), 40, "index-pack output: {stdout:?}");

    // 收到的 pack 里的对象与源仓库**逐字节相同**（hash-object 的真值来自 git 本人）。
    assert_eq!(
        git_text(&fresh, &["cat-file", "-t", &head.to_hex()]),
        "commit"
    );
    assert_eq!(
        git_text(&fresh, &["cat-file", "-p", &head.to_hex()]),
        git_text(&src, &["cat-file", "-p", &head.to_hex()])
    );
    assert_eq!(git_text(&fresh, &["cat-file", "-t", &tag.to_hex()]), "tag");
    assert_eq!(
        git_text(&fresh, &["cat-file", "-p", &tag.to_hex()]),
        git_text(&src, &["cat-file", "-p", &tag.to_hex()])
    );
    // 子目录里的 blob 也必须原样到达（这里的 `beta\n` 是夹具 `sub/b.txt` 的完整字节）。
    let blob = git_text(&src, &["rev-parse", "HEAD:sub/b.txt"]);
    assert_eq!(git_ok(&fresh, &["cat-file", "-p", &blob]).stdout, b"beta\n");
}

/// `build_push_update` 的字节 + 裸 pack 必须被真实 `git receive-pack` 接受；
/// `parse_report_status` 必须与真实服务端的结论一致（`ok` 与 `ng` 两种都要对）。
#[test]
fn our_push_update_round_trips_through_real_git_receive_pack() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = source_repo(dir.path());
    let bare = bare_repo(dir.path(), "remote.git");

    let head_hex = git_text(&src, &["rev-parse", "HEAD"]);
    let head = Oid::from_hex(&head_hex).expect("oid");
    let pack = git_ok(&src, &["pack-objects", "--stdout", "--all"]).stdout;
    assert!(pack.starts_with(b"PACK"));

    let cmds = vec![PushCommand {
        old: Oid::zeros(),
        new: head,
        name: "refs/heads/main".to_string(),
    }];
    let update = build_push_update(&cmds, &["report-status"]).expect("encoding the update");

    // 字节级：`<old> <new> <ref>\0<caps>\n` + flush，紧跟**裸** pack（不分帧）。
    let mut expect = frame(
        format!(
            "{} {} refs/heads/main\0report-status\n",
            Oid::zeros().to_hex(),
            head_hex
        )
        .as_bytes(),
    );
    expect.extend_from_slice(b"0000");
    assert_eq!(update, expect, "push update bytes must be exactly v0");

    let mut request = update.clone();
    request.extend_from_slice(&pack);
    let out = run_receive_pack(&bare, &request);
    assert!(
        out.status.success(),
        "real receive-pack failed: {}",
        out_text(&out.stderr)
    );
    assert_eq!(
        parse_report_status(&out.stdout).expect("real report-status parses"),
        vec![
            ("unpack".to_string(), "ok".to_string()),
            ("refs/heads/main".to_string(), "ok".to_string()),
        ]
    );
    assert_eq!(git_text(&bare, &["rev-parse", "refs/heads/main"]), head_hex);
    git_ok(&bare, &["fsck", "--no-progress"]);

    // 负例 1：声明了一个不存在的 old（CAS 失败）→ 真实服务端回 ng，引用不动。
    let bogus = vec![PushCommand {
        old: Oid::from_hex(&"11".repeat(20)).expect("oid"),
        new: head,
        name: "refs/heads/main".to_string(),
    }];
    let bad = build_push_update(&bogus, &["report-status"]).expect("encoding");
    let out = run_receive_pack(&bare, &bad);
    let status = parse_report_status(&out.stdout).expect("real ng must parse");
    assert!(
        status
            .iter()
            .any(|(name, state)| name == "refs/heads/main" && state.starts_with("ng ")),
        "expected an `ng` line, got {status:?}"
    );
    assert_eq!(
        git_text(&bare, &["rev-parse", "refs/heads/main"]),
        head_hex,
        "a rejected push must not move the ref"
    );

    // 负例 2：真实服务端的 non-fast-forward（`receive.denyNonFastForwards=true`）。
    // 这条同时验证「服务端能判 FF」以及 `parse_report_status` 对真实 ng 文本的还原。
    let older = git_text(&src, &["rev-parse", "HEAD~1"]);
    let rewind = vec![PushCommand {
        old: head,
        new: Oid::from_hex(&older).expect("oid"),
        name: "refs/heads/main".to_string(),
    }];
    git_ok(&bare, &["config", "receive.denyNonFastForwards", "true"]);
    let mut request = build_push_update(&rewind, &["report-status"]).expect("encoding");
    request.extend_from_slice(&pack);
    let out = run_receive_pack(&bare, &request);
    assert_eq!(
        parse_report_status(&out.stdout).expect("real ng must parse"),
        vec![
            ("unpack".to_string(), "ok".to_string()),
            (
                "refs/heads/main".to_string(),
                "ng non-fast-forward".to_string()
            ),
        ],
        "the real server's non-fast-forward reason must survive parsing"
    );
    assert_eq!(git_text(&bare, &["rev-parse", "refs/heads/main"]), head_hex);

    // 反面对照：**默认**配置下真实 `receive-pack` 不判 FF（作者的 §6 结论）。
    git_ok(&bare, &["config", "--unset", "receive.denyNonFastForwards"]);
    let mut request = build_push_update(&rewind, &["report-status"]).expect("encoding");
    request.extend_from_slice(&pack);
    let out = run_receive_pack(&bare, &request);
    assert_eq!(
        parse_report_status(&out.stdout).expect("parses"),
        vec![
            ("unpack".to_string(), "ok".to_string()),
            ("refs/heads/main".to_string(), "ok".to_string()),
        ],
        "by default the server allows non-fast-forwards, so the client must enforce it"
    );
    assert_eq!(git_text(&bare, &["rev-parse", "refs/heads/main"]), older);
}

// ---------------------------------------------------------------- B. 端到端互操作

/// `mg clone file://… dst` 的产物必须能被真实 git 完整读懂，
/// 并且与**同源的真实 `git clone`** 逐项一致。
#[test]
fn mg_clone_is_read_by_real_git_and_equals_a_real_git_clone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = source_repo(dir.path());
    let url = file_url(&src);

    let mg_dst = dir.path().join("mg-dst");
    mg_ok(dir.path(), &["clone", &url, mg_dst.to_str().unwrap()]);
    let git_dst = dir.path().join("git-dst");
    git_ok(
        dir.path(),
        &["clone", "-q", &url, git_dst.to_str().unwrap()],
    );

    // 1) 真实 git 读得懂：fsck（含 --strict）无 error，且没有输出。
    let fsck = git_ok(&mg_dst, &["fsck", "--no-progress"]);
    assert!(
        fsck.stderr.is_empty(),
        "fsck must be silent: {}",
        out_text(&fsck.stderr)
    );
    git_ok(&mg_dst, &["fsck", "--strict", "--no-progress"]);

    // 2) `git log --oneline` 与源一致；工作区/index 干净。
    assert_eq!(
        git_text(&mg_dst, &["log", "--oneline"]),
        git_text(&src, &["log", "--oneline"])
    );
    assert_eq!(git_text(&mg_dst, &["status", "--porcelain"]), "");

    // 3) 与真实 `git clone` 逐项对拍（零硬编码真值）。
    let mut mine = git_lines(&mg_dst, &["show-ref"]);
    let mut theirs = git_lines(&git_dst, &["show-ref"]);
    mine.sort();
    theirs.sort();
    assert_eq!(mine, theirs, "mg clone refs must equal a real git clone's");
    let mut mine = git_lines(&mg_dst, &["branch", "-a"]);
    let mut theirs = git_lines(&git_dst, &["branch", "-a"]);
    mine.sort();
    theirs.sort();
    assert_eq!(mine, theirs, "branches (incl. remotes) must match");
    assert_eq!(
        git_text(&mg_dst, &["symbolic-ref", "HEAD"]),
        git_text(&git_dst, &["symbolic-ref", "HEAD"])
    );
    assert_eq!(
        git_text(&mg_dst, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
        git_text(&git_dst, &["symbolic-ref", "refs/remotes/origin/HEAD"])
    );
    assert_eq!(
        git_text(&mg_dst, &["rev-list", "--all"]),
        git_text(&git_dst, &["rev-list", "--all"])
    );

    // 4) index + 工作区：模式位与内容（执行位 100755 是夹具的一部分）。
    let mut mine = git_lines(&mg_dst, &["ls-files", "-s"]);
    let mut theirs = git_lines(&src, &["ls-files", "-s"]);
    mine.sort();
    theirs.sort();
    assert_eq!(mine, theirs, "the clone's index must equal the source's");
    assert!(git_text(&mg_dst, &["ls-files", "-s", "run.sh"]).starts_with("100755"));
    assert_eq!(
        fs::read_to_string(mg_dst.join("sub/b.txt")).expect("cloned file"),
        "beta\n"
    );
    assert_eq!(
        fs::metadata(mg_dst.join("run.sh"))
            .expect("cloned exec file")
            .permissions()
            .mode()
            & 0o111,
        0o111
    );

    // 5) 配置是真实 git 能读的形态（fetch/pull 依赖它）。
    assert_eq!(
        git_text(&mg_dst, &["config", "--get", "remote.origin.url"]),
        url
    );
    assert_eq!(
        git_text(&mg_dst, &["config", "--get", "remote.origin.fetch"]),
        "+refs/heads/*:refs/remotes/origin/*"
    );
    assert_eq!(
        git_text(&mg_dst, &["config", "--get", "branch.main.remote"]),
        "origin"
    );
    assert_eq!(
        git_text(&mg_dst, &["config", "--get", "branch.main.merge"]),
        "refs/heads/main"
    );
    // 6) tags：真实 git 看得到两个 tag，且类型正确。
    assert_eq!(
        git_text(&mg_dst, &["cat-file", "-t", "refs/tags/v1"]),
        "tag"
    );
    assert_eq!(
        git_text(&mg_dst, &["cat-file", "-t", "refs/tags/light"]),
        "commit"
    );
    // 7) 反向：真实 git 能在 mg 写的仓库里工作（提交 + 用该仓库的 remote 推回裸仓库）。
    write_file(&mg_dst, "by-git.txt", b"from git\n");
    commit_all(&mg_dst, "committed by real git");
    git_ok(&mg_dst, &["fsck", "--no-progress"]);
    assert!(git_text(&mg_dst, &["status", "--porcelain"]).is_empty());
}

/// `mg fetch`（库 API）→ 远端跟踪引用与 `FETCH_HEAD` 正确；
/// **第二次 fetch 不再传对象**（说明 `have` 行真的参与了协商，不是每次全量照搬）。
#[test]
fn mg_fetch_updates_remote_tracking_refs_and_negotiates_haves() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = source_repo(dir.path());
    let url = file_url(&src);
    let dst = dir.path().join("dst");
    mg_ok(dir.path(), &["clone", &url, dst.to_str().unwrap()]);

    // 源仓库前进（真实 git 提交）。
    write_file(&src, "c.txt", b"gamma\n");
    commit_all(&src, "three");
    let remote_head = git_text(&src, &["rev-parse", "refs/heads/main"]);

    let repo = Repo::discover(&dst).expect("the clone is a repository");
    let refspec = "+refs/heads/*:refs/remotes/origin/*".to_string();
    let first =
        local::fetch(&repo, &url, std::slice::from_ref(&refspec)).expect("fetch must succeed");
    assert!(
        first.objects_written > 0,
        "the new commit must be transferred"
    );
    let store = RefStore::new(&repo);
    assert_eq!(
        store
            .resolve("refs/remotes/origin/main")
            .expect("remote-tracking ref")
            .to_hex(),
        remote_head,
        "refs/remotes/origin/main must equal the source's refs/heads/main"
    );
    assert_eq!(
        first.head.map(|(_, oid)| oid.to_hex()).as_deref(),
        Some(remote_head.as_str())
    );

    // FETCH_HEAD：与**真实 `git fetch`（同一 refspec、同一 url）**的 FETCH_HEAD 对拍。
    // （FETCH_HEAD 记录的是 `<oid>\t\t[not-for-merge\t]branch 'x' of <url>`，
    //   不含 `refs/remotes/...` 目标名 —— 这正是本轮要自证的真值来源之一。）
    let ref_dst = dir.path().join("ref-dst");
    git_ok(
        dir.path(),
        &["clone", "-q", &url, ref_dst.to_str().unwrap()],
    );
    git_ok(
        &ref_dst,
        &["fetch", "origin", "+refs/heads/*:refs/remotes/origin/*"],
    );
    let mine = fetch_head_oids(&dst);
    let real = fetch_head_oids(&ref_dst);
    assert_eq!(
        mine, real,
        "mg's FETCH_HEAD oids must equal a real `git fetch`'s for the same refspec"
    );
    assert!(
        mine.contains(&remote_head),
        "FETCH_HEAD must mention the fetched head: {mine:?}"
    );

    // 第二次 fetch：want 已经全在本地，`have` 行生效 → 0 个对象。
    let second = local::fetch(&repo, &url, &[refspec]).expect("second fetch");
    assert_eq!(
        second.objects_written, 0,
        "a second fetch must not re-transfer objects (that would mean `have` is ignored)"
    );

    // 用 `mg` 二进制再走一遍（另一个新提交），确认 CLI 与库同源。
    write_file(&src, "d.txt", b"delta\n");
    commit_all(&src, "four");
    let remote_head = git_text(&src, &["rev-parse", "refs/heads/main"]);
    mg_ok(&dst, &["fetch", "origin"]);
    assert_eq!(
        git_text(&dst, &["rev-parse", "refs/remotes/origin/main"]),
        remote_head
    );
    assert!(dst.join(".git/FETCH_HEAD").exists());
    assert!(git_text(&dst, &["status", "--porcelain"]).is_empty());
}

/// `mg push` 到 `file://` 远端：真实 git 看得到新提交；
/// non-fast-forward 必须**非零退出且远端逐字节不变**；`--force` 才移动。
#[test]
fn mg_push_reaches_real_git_and_non_fast_forward_leaves_the_remote_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bare = bare_repo(dir.path(), "remote.git");
    let url = file_url(&bare);

    // 远端初始状态：用真实 git 推一个带历史的 main。
    let seed = source_repo(dir.path());
    git_ok(
        &seed,
        &[
            "push",
            "-q",
            bare.to_str().unwrap(),
            "refs/heads/main:refs/heads/main",
        ],
    );
    let head1 = git_text(&bare, &["rev-parse", "refs/heads/main"]);
    assert_eq!(head1, git_text(&seed, &["rev-parse", "refs/heads/main"]));

    let work = dir.path().join("work");
    mg_ok(dir.path(), &["clone", &url, work.to_str().unwrap()]);
    write_file(&work, "local.txt", b"local\n");
    commit_all(&work, "local work");
    let head2 = git_text(&work, &["rev-parse", "HEAD"]);
    assert_ne!(head2, head1);

    mg_ok(&work, &["push", "origin", "main"]);
    assert_eq!(
        git_text(&bare, &["rev-parse", "refs/heads/main"]),
        head2,
        "real git must see the pushed commit"
    );
    assert!(
        git_lines(&bare, &["log", "--oneline"])
            .iter()
            .any(|line| line.ends_with("local work")),
        "the pushed commit must be in the remote's history"
    );
    git_ok(&bare, &["fsck", "--no-progress"]);

    // 让远端前进到一个**分叉**提交（真实 git 从同一远端再克隆）。
    let other = dir.path().join("other");
    git_ok(dir.path(), &["clone", "-q", &url, other.to_str().unwrap()]);
    write_file(&other, "other.txt", b"other\n");
    commit_all(&other, "other work");
    git_ok(&other, &["push", "-q", "origin", "main"]);
    let head3 = git_text(&bare, &["rev-parse", "refs/heads/main"]);
    assert_ne!(head3, head2);

    // non-fast-forward：`work` 的 head2 与远端的 head3 已分叉。
    let before_digest = repo_digest(&bare);
    let before_head = git_text(&bare, &["rev-parse", "refs/heads/main"]);
    let stderr = mg_fail(&work, &["push", "origin", "main"]);
    assert!(
        stderr.contains("fast-forward"),
        "the rejection must explain itself: {stderr}"
    );
    assert_eq!(
        git_text(&bare, &["rev-parse", "refs/heads/main"]),
        before_head,
        "a refused push must leave the remote ref alone"
    );
    assert_eq!(
        repo_digest(&bare),
        before_digest,
        "a refused push must not even write objects into the remote"
    );

    // `--force` 才移动远端。
    mg_ok(&work, &["push", "--force", "origin", "main"]);
    assert_eq!(git_text(&bare, &["rev-parse", "refs/heads/main"]), head2);
    git_ok(&bare, &["fsck", "--no-progress"]);
}

/// 库 API 的 push 与 CLI 同源：`local::push` 能新建远端分支（真实 git 看得到）。
#[test]
fn library_push_creates_a_new_remote_branch_visible_to_real_git() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bare = bare_repo(dir.path(), "remote.git");
    let url = file_url(&bare);
    let work = dir.path().join("work");

    // 先做一个空远端上的首个 push：库 API 直接调用。
    let repo = Repo::init(&work, "main").expect("mg-side init");
    write_file(&work, "a.txt", b"alpha\n");
    // 用真实 git 在 mg 建的仓库里提交（真实 git 必须能写 mg 建的仓库）。
    commit_all(&work, "first");
    let head = git_text(&work, &["rev-parse", "HEAD"]);
    local::push(&repo, &url, &["main".to_string()], false).expect("push to an empty bare remote");
    assert_eq!(git_text(&bare, &["rev-parse", "refs/heads/main"]), head);
    git_ok(&bare, &["fsck", "--no-progress"]);

    // 新建远端分支：真实 git 也能看到。
    git_ok(&work, &["branch", "feature"]);
    local::push(&repo, &url, &["feature".to_string()], false).expect("push a new branch");
    assert_eq!(git_text(&bare, &["rev-parse", "refs/heads/feature"]), head);
    let refs = git_lines(&bare, &["for-each-ref", "--format=%(refname)"]);
    assert!(refs.contains(&"refs/heads/main".to_string()), "{refs:?}");
    assert!(refs.contains(&"refs/heads/feature".to_string()), "{refs:?}");
    git_ok(&bare, &["fsck", "--no-progress"]);
}

/// `mg pull` 的快进与真实 `git pull` 结果一致（HEAD / 工作区 / status / fsck）。
#[test]
fn mg_pull_fast_forwards_like_real_git() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bare = bare_repo(dir.path(), "remote.git");
    let url = file_url(&bare);
    let seed = source_repo(dir.path());
    git_ok(
        &seed,
        &[
            "push",
            "-q",
            bare.to_str().unwrap(),
            "refs/heads/main:refs/heads/main",
        ],
    );

    let mine = dir.path().join("mine");
    let theirs = dir.path().join("theirs");
    mg_ok(dir.path(), &["clone", &url, mine.to_str().unwrap()]);
    git_ok(dir.path(), &["clone", "-q", &url, theirs.to_str().unwrap()]);

    // 远端前进。
    let other = dir.path().join("other");
    git_ok(dir.path(), &["clone", "-q", &url, other.to_str().unwrap()]);
    write_file(&other, "pulled.txt", b"pulled\n");
    commit_all(&other, "remote advance");
    git_ok(&other, &["push", "-q", "origin", "main"]);
    let remote_head = git_text(&bare, &["rev-parse", "refs/heads/main"]);

    mg_ok(&mine, &["pull", "origin"]);
    git_ok(&theirs, &["pull", "-q", "origin", "main"]);

    assert_eq!(git_text(&mine, &["rev-parse", "HEAD"]), remote_head);
    assert_eq!(
        git_text(&mine, &["rev-parse", "HEAD"]),
        git_text(&theirs, &["rev-parse", "HEAD"]),
        "mg pull must land on the same commit as real git pull"
    );
    assert_eq!(
        fs::read_to_string(mine.join("pulled.txt")).expect("pulled file"),
        fs::read_to_string(theirs.join("pulled.txt")).expect("pulled file")
    );
    assert_eq!(git_text(&mine, &["status", "--porcelain"]), "");
    assert_eq!(
        git_text(&mine, &["status", "--porcelain"]),
        git_text(&theirs, &["status", "--porcelain"])
    );
    assert_eq!(
        git_text(&mine, &["log", "--oneline"]),
        git_text(&theirs, &["log", "--oneline"])
    );
    git_ok(&mine, &["fsck", "--no-progress"]);
    // 第二次 pull：真实 git 会说 already up to date，mg 也必须是空操作。
    let again = mg_ok(&mine, &["pull", "origin"]);
    assert!(
        out_text(&again.stdout)
            .to_lowercase()
            .contains("up to date"),
        "stdout: {}",
        out_text(&again.stdout)
    );
    assert_eq!(git_text(&mine, &["rev-parse", "HEAD"]), remote_head);
}

// ---------------------------------------------------------------- C. 反例与边界

/// 失败路径：清晰 `Err`、非零退出、不 panic、**不留下半成品目录**。
#[test]
fn clone_and_fetch_failures_are_clean_and_leave_nothing_behind() {
    let dir = tempfile::tempdir().expect("tempdir");

    // 1) url 不存在（库 API + CLI 两条路都要干净）。
    let missing = dir.path().join("nope");
    let url = file_url(&missing);
    let dst = dir.path().join("d1");
    let err = local::clone_into(&url, &dst).expect_err("a missing remote must fail");
    assert!(
        err.to_string().contains("does not exist"),
        "unexpected error: {err}"
    );
    assert!(!dst.exists(), "a failed clone must not leave a directory");
    let stderr = mg_fail(dir.path(), &["clone", &url, dst.to_str().unwrap()]);
    assert!(stderr.contains("does not exist"), "{stderr}");
    assert!(!dst.exists(), "a failed clone must not leave a directory");

    // 2) 存在但不是仓库。
    let not_repo = dir.path().join("not-a-repo");
    fs::create_dir_all(&not_repo).expect("dir");
    write_file(&not_repo, "plain.txt", b"not a repo\n");
    let dst = dir.path().join("d2");
    let stderr = mg_fail(
        dir.path(),
        &["clone", &file_url(&not_repo), dst.to_str().unwrap()],
    );
    assert!(stderr.contains("not a git repository"), "{stderr}");
    assert!(!dst.exists());
    assert!(
        not_repo.join("plain.txt").exists(),
        "the remote must be untouched"
    );

    // 3) 空仓库（git init、无提交）—— **对齐真实 git**：
    //    真实 git 2.55.0 在空远端上是 `warning: You appear to have cloned an empty repository.`
    //    + exit 0，并留下一个只含 `.git` 的克隆。
    //    历史：V13 曾断言「必须报错且不留目录」，controller 在 W4/C-28 裁决跟随真实 git，
    //    T13c 实现后把本断言翻转为「与 git 同向」。
    let empty = dir.path().join("empty");
    fs::create_dir_all(&empty).expect("dir");
    git_ok(&empty, &["init", "-q", "-b", "main", "."]);
    let dst = dir.path().join("d3");
    // 真实 git 的对照产物
    let git_dst = dir.path().join("d3-git");
    git_ok(
        dir.path(),
        &["clone", &file_url(&empty), git_dst.to_str().unwrap()],
    );
    let out = mg_ok(
        dir.path(),
        &["clone", &file_url(&empty), dst.to_str().unwrap()],
    );
    let our_warning = out_text(&out.stderr);
    assert!(
        our_warning.contains("You appear to have cloned an empty repository"),
        "must print the same warning as real git, got {our_warning:?}"
    );
    assert!(
        dst.exists(),
        "an empty clone still creates the target directory"
    );
    assert!(
        dst.join(".git").exists(),
        "the empty clone must be a real repository"
    );
    // 与真实 git 的产物逐字段比对：无引用、无工作区文件、HEAD 指向同一分支。
    // 注：空仓库上 `git show-ref` 以 exit 1 结束（没有任何引用），所以这里不能用 `git_text`。
    let ours = git_text(&dst, &["symbolic-ref", "HEAD"]);
    let theirs = git_text(&git_dst, &["symbolic-ref", "HEAD"]);
    assert_eq!(ours, theirs, "symbolic-ref HEAD must match real git");
    let ours = out_text(&git_raw(&dst, &["show-ref"]).stdout);
    let theirs = out_text(&git_raw(&git_dst, &["show-ref"]).stdout);
    assert_eq!(ours, "", "an empty clone advertises no refs");
    assert_eq!(ours, theirs, "show-ref must match real git");
    let ours = git_text(&dst, &["status", "--porcelain"]);
    assert_eq!(ours, "", "an empty clone has a clean worktree");

    // 4) 目标目录已存在且非空 → 拒绝，且原文件原样保留。
    let occupied = dir.path().join("occupied");
    fs::create_dir_all(&occupied).expect("dir");
    write_file(&occupied, "keep.txt", b"keep me\n");
    let src = source_repo(dir.path());
    let stderr = mg_fail(
        dir.path(),
        &["clone", &file_url(&src), occupied.to_str().unwrap()],
    );
    assert!(
        stderr.contains("already exists") || stderr.contains("not an empty"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(occupied.join("keep.txt")).expect("kept file"),
        "keep me\n"
    );

    // 5) 只读父目录（chmod 0o555）→ 建目录失败、清晰报错、无残留。
    let ro = dir.path().join("ro");
    fs::create_dir_all(&ro).expect("dir");
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).expect("chmod");
    let dst = ro.join("d5");
    let stderr = mg_fail(
        dir.path(),
        &["clone", &file_url(&src), dst.to_str().unwrap()],
    );
    assert!(
        !stderr.is_empty(),
        "a read-only destination must report an error"
    );
    assert!(!dst.exists());
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o755)).expect("chmod back");

    // 6) 不在仓库里跑 fetch/push/pull → 统一的「不是仓库」错误，不 panic。
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).expect("dir");
    for args in [
        vec!["fetch", "origin"],
        vec!["push", "origin", "main"],
        vec!["pull", "origin"],
    ] {
        let stderr = mg_fail(&outside, &args);
        assert!(
            stderr.contains("not a git repository"),
            "{args:?}: {stderr}"
        );
    }

    // 7) 目标是一个文件（不是目录）。
    let as_file = dir.path().join("a-file");
    write_file(dir.path(), "a-file", b"x\n");
    let stderr = mg_fail(
        dir.path(),
        &["clone", &file_url(&src), as_file.to_str().unwrap()],
    );
    assert!(stderr.contains("not a directory"), "{stderr}");
    assert_eq!(fs::read_to_string(&as_file).expect("file"), "x\n");
}

/// URL 路由：`file://` 走本地路径，`https://` / `git://` / 带主机的 `file://` 明确 Unsupported；
/// 没有 scheme 的本地路径按路径处理。`http://` 属 T14，本文件不改写它的行为、只做「不被 file 分支吃掉」的检查。
#[test]
fn url_routing_keeps_non_file_schemes_out_of_the_file_path_branch() {
    let dir = tempfile::tempdir().expect("tempdir");

    // file:// + 远端主机名 → Unsupported（不能当成 `/host/path` 的本地路径）。
    let stderr = mg_fail(dir.path(), &["clone", "file://example.com/tmp/x", "d-host"]);
    assert!(stderr.contains("host"), "{stderr}");
    assert!(!dir.path().join("d-host").exists());

    // https:// → Unsupported（v1 不引 TLS）。
    let stderr = mg_fail(
        dir.path(),
        &["clone", "https://example.com/x.git", "d-https"],
    );
    assert!(stderr.to_lowercase().contains("https"), "{stderr}");
    assert!(!dir.path().join("d-https").exists());

    // git:// → Unsupported。
    let stderr = mg_fail(dir.path(), &["clone", "git://example.com/x", "d-git"]);
    assert!(
        stderr.contains("git://") || stderr.contains("TCP"),
        "{stderr}"
    );
    assert!(!dir.path().join("d-git").exists());

    // 没有 scheme 的本地路径按路径处理（真实 git 也这样）→ 报「不存在」，而不是「不支持的 scheme」。
    let stderr = mg_fail(
        dir.path(),
        &["clone", "/nonexistent/mini-git-verify", "d-plain"],
    );
    assert!(stderr.contains("does not exist"), "{stderr}");

    // http:// 必须**不**落进 file 路径分支（T14 覆盖，本文件只检查路由）：
    // 报错文本不能是「file 路径不存在」那种形态。
    let stderr = mg_fail(dir.path(), &["clone", "http://127.0.0.1:1/x.git", "d-http"]);
    assert!(
        !stderr.contains("remote repository 'http"),
        "http:// must not be treated as a local path: {stderr}"
    );
    assert!(!dir.path().join("d-http").exists());
}
