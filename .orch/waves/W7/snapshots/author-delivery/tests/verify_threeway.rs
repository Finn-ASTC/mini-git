//! V10 —— 独立验证 T10（文件级三方合并 `merge_blobs` + `mg merge`）。
//!
//! 纪律（对应任务书 §2/§3/§4）：
//! * **真值唯一来源是运行时真实 `git` 进程与文件系统**：`git merge-file`（引擎级）、
//!   `git merge`（CLI 级平行仓库对拍）、`git rev-parse` / `git status --porcelain` /
//!   `git ls-files --stage` / `git fsck` / `git log --format=%P`。本文件不硬编码任何
//!   期望字节、oid 或冲突文本。
//! * **反假绿**：引擎级断言必须真的调 `minigit::merge::three_way::merge_blobs`；
//!   CLI 级断言必须真的跑 `env!("CARGO_BIN_EXE_mg")`。git 的输出只作为「期望」，
//!   绝不回灌成被测函数的返回值。
//! * **平行仓库对拍**：同一初始状态 `cp -a` 两份，A 用 `mg merge`、B 用真实
//!   `git merge`，逐项比较 tree / 工作区字节 / `git status --porcelain` /
//!   `git ls-files --stage` / `git fsck` / 冲突后真实 `git commit` 的两父提交。
//!
//! 真值来源自证（§4）：见文件末尾 `truth_source_selfcheck_*` 三个用例。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use minigit::merge::three_way::{merge_blobs, MergeLabels, Merged};

// ----------------------------------------------------------------- 进程工具

fn mg_bin() -> &'static str {
    env!("CARGO_BIN_EXE_mg")
}

/// 真实 git：身份/配置/对象库环境全部就地隔离，绝不 export 进共享 shell。
fn git_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.args([
        "-c",
        "user.name=V10 verifier",
        "-c",
        "user.email=v10@example.invalid",
        "-c",
        "core.autocrlf=false",
        "-c",
        "core.safecrlf=false",
    ])
    .current_dir(dir)
    .env("LC_ALL", "C")
    .env("LANG", "C")
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GIT_CONFIG_SYSTEM", "/dev/null")
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env_remove("GIT_DIR")
    .env_remove("GIT_WORK_TREE")
    .env_remove("GIT_INDEX_FILE")
    .env_remove("GIT_OBJECT_DIRECTORY")
    .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
    .stdin(Stdio::null());
    cmd
}

/// 被测 CLI：`mg -C <repo> <args...>`。
fn mg_cmd(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(mg_bin());
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .stdin(Stdio::null());
    cmd
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("spawn process")
}

fn out_text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn git(dir: &Path, args: &[&str]) -> Output {
    run(git_cmd(dir).args(args))
}

fn git_ok(dir: &Path, args: &[&str]) -> String {
    let out = git(dir, args);
    assert!(
        out.status.success(),
        "git {args:?} in {} failed ({}): {}",
        dir.display(),
        code(&out),
        out_text(&out)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn mg(dir: &Path, args: &[&str]) -> Output {
    run(&mut mg_cmd(dir, args))
}

// ------------------------------------------------------------ 文件系统工具

fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, bytes).expect("write file");
}

/// 递归快照：相对路径 → 字节内容（跳过 `.git`）。
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    walk(root, Path::new(""), &mut out);
    out
}

fn walk(root: &Path, rel: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let dir = root.join(rel);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let child = rel.join(&name);
        let meta = fs::symlink_metadata(entry.path()).expect("symlink_metadata");
        if meta.is_dir() {
            walk(root, &child, out);
        } else if meta.file_type().is_symlink() {
            let target = fs::read_link(entry.path()).expect("readlink");
            out.insert(
                child.to_string_lossy().to_string(),
                format!("symlink:{}", target.display()).into_bytes(),
            );
        } else {
            out.insert(
                child.to_string_lossy().to_string(),
                fs::read(entry.path()).expect("read file"),
            );
        }
    }
}

fn copy_tree(src: &Path, dst: &Path) {
    let out = run(Command::new("cp").arg("-a").arg(src).arg(dst));
    assert!(
        out.status.success(),
        "cp -a {} {} failed: {}",
        src.display(),
        dst.display(),
        out_text(&out)
    );
}

/// 仓库内全部文件的 sha256 清单（系统 `sha256sum`，与任务书 §2(B)4 一致）。
fn sha_manifest(repo: &Path) -> String {
    let script = "find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum";
    let out = run(Command::new("bash").arg("-c").arg(script).current_dir(repo));
    assert!(out.status.success(), "sha256sum sweep failed");
    String::from_utf8_lossy(&out.stdout).to_string()
}

// --------------------------------------------------------------- 场景构造器

/// 一个「平行仓库对拍」结果：两份同源仓库 + 两条命令的输出。
struct Pair {
    _root: tempfile::TempDir,
    tag: String,
    mg_repo: PathBuf,
    git_repo: PathBuf,
    mg_out: Output,
    git_out: Output,
}

#[derive(Clone, Copy)]
enum Ch {
    Set(&'static str, &'static [u8]),
    Del(&'static str),
}

fn apply(repo: &Path, changes: &[Ch]) {
    for change in changes {
        match change {
            Ch::Set(path, bytes) => write_file(&repo.join(path), bytes),
            Ch::Del(path) => {
                let _ = fs::remove_file(repo.join(path));
            }
        }
    }
}

/// base 提交 → `feature` 上打 theirs → 回 `main` 打 ours。
fn history(repo: &Path, base: &[Ch], ours: &[Ch], theirs: &[Ch]) {
    apply(repo, base);
    git_ok(repo, &["add", "-A"]);
    git_ok(repo, &["commit", "-qm", "base"]);
    git_ok(repo, &["checkout", "-qb", "feature"]);
    apply(repo, theirs);
    git_ok(repo, &["add", "-A"]);
    git_ok(repo, &["commit", "-qm", "theirs"]);
    git_ok(repo, &["checkout", "-q", "main"]);
    apply(repo, ours);
    git_ok(repo, &["add", "-A"]);
    git_ok(repo, &["commit", "-qm", "ours"]);
}

fn new_repo(dir: &Path) {
    fs::create_dir_all(dir).expect("mkdir repo");
    git_ok(dir, &["init", "-q", "-b", "main", "."]);
    git_ok(dir, &["config", "user.name", "V10 verifier"]);
    git_ok(dir, &["config", "user.email", "v10@example.invalid"]);
}

/// 造一份初始状态，`cp -a` 两份，A 跑 `mg <mg_args>`、B 跑 `git <git_args>`。
fn pair(tag: &str, setup: &dyn Fn(&Path), mg_args: &[&str], git_args: &[&str]) -> Pair {
    let root = tempfile::tempdir().expect("tempdir");
    let base = root.path().join("base");
    new_repo(&base);
    setup(&base);
    let mg_repo = root.path().join("mg-repo");
    let git_repo = root.path().join("git-repo");
    copy_tree(&base, &mg_repo);
    copy_tree(&base, &git_repo);
    let mg_out = mg(&mg_repo, mg_args);
    let git_out = git(&git_repo, git_args);
    Pair {
        _root: root,
        tag: tag.to_string(),
        mg_repo,
        git_repo,
        mg_out,
        git_out,
    }
}

impl Pair {
    fn tree_mg(&self) -> String {
        git_ok(&self.mg_repo, &["rev-parse", "HEAD^{tree}"])
    }
    fn tree_git(&self) -> String {
        git_ok(&self.git_repo, &["rev-parse", "HEAD^{tree}"])
    }
    fn head_mg(&self) -> String {
        git_ok(&self.mg_repo, &["rev-parse", "HEAD"])
    }
    fn head_git(&self) -> String {
        git_ok(&self.git_repo, &["rev-parse", "HEAD"])
    }
    fn porcelain_mg(&self) -> String {
        git_ok(&self.mg_repo, &["status", "--porcelain"])
    }
    fn porcelain_git(&self) -> String {
        git_ok(&self.git_repo, &["status", "--porcelain"])
    }
    fn stage_mg(&self) -> String {
        git_ok(&self.mg_repo, &["ls-files", "--stage"])
    }
    fn stage_git(&self) -> String {
        git_ok(&self.git_repo, &["ls-files", "--stage"])
    }
    fn fsck(&self, repo: &Path) {
        let out = git(repo, &["fsck", "--no-progress"]);
        let text = out_text(&out);
        assert!(
            !text.contains("error") && !text.contains("fatal"),
            "{}: git fsck reported problems: {text}",
            self.tag
        );
    }
    fn parents(&self, repo: &Path) -> Vec<String> {
        git_ok(repo, &["log", "--format=%P", "-1"])
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }
    fn subject(&self, repo: &Path) -> String {
        git_ok(repo, &["log", "--format=%s", "-1"])
    }

    /// 干净合并的全部不变量。
    fn assert_clean(&self) {
        assert!(
            self.mg_out.status.success(),
            "{}: `mg merge` should succeed, got exit {} / {}",
            self.tag,
            code(&self.mg_out),
            out_text(&self.mg_out)
        );
        assert!(
            self.git_out.status.success(),
            "{}: `git merge` oracle failed: {}",
            self.tag,
            out_text(&self.git_out)
        );
        assert_eq!(
            self.tree_mg(),
            self.tree_git(),
            "{}: HEAD^{{tree}} differs from real git's merge",
            self.tag
        );
        assert_eq!(
            snapshot(&self.mg_repo),
            snapshot(&self.git_repo),
            "{}: working tree bytes differ from real git's merge",
            self.tag
        );
        assert_eq!(
            self.porcelain_mg(),
            self.porcelain_git(),
            "{}: git status --porcelain differs",
            self.tag
        );
        assert_eq!(
            self.stage_mg(),
            self.stage_git(),
            "{}: git ls-files --stage differs",
            self.tag
        );
        assert_eq!(
            self.parents(&self.mg_repo).len(),
            2,
            "{}: merge commit must have two parents",
            self.tag
        );
        assert_eq!(
            self.subject(&self.mg_repo),
            self.subject(&self.git_repo),
            "{}: merge commit subject differs",
            self.tag
        );
        self.fsck(&self.mg_repo);
        self.fsck(&self.git_repo);
    }

    /// 冲突合并的全部不变量（含「真实 git 能在这份状态上收尾」）。
    fn assert_conflict(&self, conflict_paths: &[&str]) {
        assert!(
            !self.mg_out.status.success(),
            "{}: conflicting merge must exit non-zero, got 0",
            self.tag
        );
        assert!(
            !self.git_out.status.success(),
            "{}: git merge oracle unexpectedly succeeded",
            self.tag
        );
        for path in conflict_paths {
            let a = fs::read(self.mg_repo.join(path)).expect("mg conflict file");
            let b = fs::read(self.git_repo.join(path)).expect("git conflict file");
            assert_eq!(
                a,
                b,
                "{}: {path} conflict bytes differ\nmg = {:?}\ngit= {:?}",
                self.tag,
                String::from_utf8_lossy(&a),
                String::from_utf8_lossy(&b)
            );
        }
        assert_eq!(
            snapshot(&self.mg_repo),
            snapshot(&self.git_repo),
            "{}: working tree bytes differ after conflicting merge",
            self.tag
        );
        assert_eq!(
            self.porcelain_mg(),
            self.porcelain_git(),
            "{}: conflict codes in git status --porcelain differ",
            self.tag
        );
        assert_eq!(
            self.stage_mg(),
            self.stage_git(),
            "{}: index stages differ (stage1/2/3 must match real git)",
            self.tag
        );
        self.fsck(&self.mg_repo);
        self.fsck(&self.git_repo);

        // MERGE_HEAD 必须是 target oid 的十六进制 + 换行。
        let target = git_ok(&self.git_repo, &["rev-parse", "feature"]);
        assert_eq!(
            fs::read_to_string(self.mg_repo.join(".git/MERGE_HEAD")).expect("MERGE_HEAD"),
            format!("{target}\n"),
            "{}: .git/MERGE_HEAD must be hex oid + newline",
            self.tag
        );
        let orig = fs::read_to_string(self.mg_repo.join(".git/ORIG_HEAD")).expect("ORIG_HEAD");
        assert_eq!(
            orig.len(),
            41,
            "{}: ORIG_HEAD must be hex oid + newline",
            self.tag
        );
        assert!(orig.ends_with('\n'));
        assert!(
            !fs::read_to_string(self.mg_repo.join(".git/MERGE_MSG"))
                .expect("MERGE_MSG")
                .is_empty(),
            "{}: .git/MERGE_MSG must not be empty",
            self.tag
        );

        // 硬标准：真实 git 在 mg 留下的状态上完成合并。
        let resolved: Vec<(String, Vec<u8>)> = conflict_paths
            .iter()
            .map(|p| (p.to_string(), resolve_bytes(&self.mg_repo.join(p))))
            .collect();
        for (path, bytes) in &resolved {
            write_file(&self.mg_repo.join(path), bytes);
            write_file(&self.git_repo.join(path), bytes);
        }
        for repo in [&self.mg_repo, &self.git_repo] {
            git_ok(repo, &["add", "-A"]);
            let out = git(repo, &["commit", "--no-edit", "-q"]);
            assert!(
                out.status.success(),
                "{}: real `git commit` could not finish the merge in {}: {}",
                self.tag,
                repo.display(),
                out_text(&out)
            );
            let parents = self.parents(repo);
            assert_eq!(
                parents.len(),
                2,
                "{}: post-conflict commit must have two parents",
                self.tag
            );
        }
        assert_eq!(
            self.tree_mg(),
            self.tree_git(),
            "{}: tree after resolving conflicts differs from real git",
            self.tag
        );
        assert_eq!(self.porcelain_mg(), "", "{}: dirty after commit", self.tag);
        self.fsck(&self.mg_repo);
    }
}

/// 把 mg 写出的冲突文件「解决」成确定内容（丢弃 marker 行）。
fn resolve_bytes(path: &Path) -> Vec<u8> {
    let bytes = fs::read(path).expect("read conflict file");
    let text = String::from_utf8_lossy(&bytes).to_string();
    let mut out = String::new();
    for line in text.lines() {
        let trimmed = line.trim_end_matches('\r');
        if trimmed.starts_with("<<<<<<<")
            || trimmed.starts_with("=======")
            || trimmed.starts_with(">>>>>>>")
            || trimmed.starts_with("|||||||")
        {
            continue;
        }
        out.push_str("RESOLVED\n");
    }
    out.into_bytes()
}

// ============================================================ (B1) 引擎级

fn labels(diff3: bool) -> MergeLabels {
    MergeLabels {
        ours: "ours".to_string(),
        base: "base".to_string(),
        theirs: "theirs".to_string(),
        diff3_style: diff3,
    }
}

/// 真值：`git merge-file -p [-L ours -L base -L theirs] ours base theirs`。
/// `None` 表示真实 git 拒绝合并（例如二进制，退出码 255）。
fn truth_merge_file(
    dir: &Path,
    ours: &[u8],
    base: &[u8],
    theirs: &[u8],
    diff3: bool,
) -> Option<(Vec<u8>, i32)> {
    write_file(&dir.join("t_ours"), ours);
    write_file(&dir.join("t_base"), base);
    write_file(&dir.join("t_theirs"), theirs);
    let mut args: Vec<&str> = vec!["merge-file", "-p"];
    if diff3 {
        args.push("--diff3");
    }
    args.extend([
        "-L", "ours", "-L", "base", "-L", "theirs", "t_ours", "t_base", "t_theirs",
    ]);
    let out = run(git_cmd(dir).args(&args));
    let exit = code(&out);
    if !(0..128).contains(&exit) {
        return None;
    }
    Some((out.stdout, exit))
}

/// 引擎级对拍：`merge_blobs` 的结果必须与 `git merge-file` **逐字节**相同，
/// 且 Clean/Conflict 分类必须与 `git merge-file` 的退出码（= 冲突数）一致。
/// 返回该用例是否冲突。
fn engine_case(name: &str, ours: &[u8], base: &[u8], theirs: &[u8], diff3: bool) -> bool {
    let dir = tempfile::tempdir().expect("tempdir");
    let Some((truth, conflicts)) = truth_merge_file(dir.path(), ours, base, theirs, diff3) else {
        panic!("{name}: real git merge-file refused this input (binary?)");
    };
    let merged = merge_blobs(Some(base), ours, theirs, &labels(diff3)).expect("merge_blobs");
    let (variant, bytes) = match &merged {
        Merged::Clean(bytes) => ("Clean", bytes),
        Merged::Conflict(bytes) => ("Conflict", bytes),
    };
    assert_eq!(
        bytes.as_slice(),
        truth.as_slice(),
        "{name} (diff3={diff3}): merge_blobs bytes differ from `git merge-file`\nmg  = {:?}\ngit = {:?}",
        String::from_utf8_lossy(bytes),
        String::from_utf8_lossy(&truth)
    );
    assert_eq!(
        variant,
        if conflicts > 0 { "Conflict" } else { "Clean" },
        "{name}: variant must match git merge-file exit code {conflicts}"
    );
    conflicts > 0
}

/// 一个引擎级语料项：`(名字, ours, base, theirs)`。
type EngineCase = (&'static str, Vec<u8>, Vec<u8>, Vec<u8>);

#[test]
fn engine_case_table_matches_git_merge_file() {
    let empty: &[u8] = b"";
    let cases: Vec<EngineCase> = vec![
        (
            "disjoint-changes",
            b"1\nOURS\n3\n4\n5\n".to_vec(),
            b"1\n2\n3\n4\n5\n".to_vec(),
            b"1\n2\n3\n4\nTHEIRS\n".to_vec(),
        ),
        (
            "same-line-both-changed",
            b"1\nOURS\n3\n".to_vec(),
            b"1\n2\n3\n".to_vec(),
            b"1\nTHEIRS\n3\n".to_vec(),
        ),
        (
            "adjacent-hunks",
            b"1\nA\nB\n4\n".to_vec(),
            b"1\n2\n3\n4\n".to_vec(),
            b"1\nX\nY\n4\n".to_vec(),
        ),
        (
            "ours-delete-one-line-theirs-modify-it",
            b"1\n3\n".to_vec(),
            b"1\n2\n3\n".to_vec(),
            b"1\nTWO\n3\n".to_vec(),
        ),
        (
            "ours-delete-block-theirs-modify-inside",
            b"1\n".to_vec(),
            b"1\n2\n3\n4\n".to_vec(),
            b"1\n2\nX\n4\n".to_vec(),
        ),
        (
            "both-add-at-same-position",
            b"1\nOURS\n2\n".to_vec(),
            b"1\n2\n".to_vec(),
            b"1\nTHEIRS\n2\n".to_vec(),
        ),
        (
            "both-add-at-different-positions",
            b"OURS\n1\n2\n".to_vec(),
            b"1\n2\n".to_vec(),
            b"1\n2\nTHEIRS\n".to_vec(),
        ),
        (
            "empty-base-both-added",
            b"ours line\n".to_vec(),
            empty.to_vec(),
            b"theirs line\n".to_vec(),
        ),
        ("all-empty", empty.to_vec(), empty.to_vec(), empty.to_vec()),
        (
            "ours-emptied-theirs-modified",
            empty.to_vec(),
            b"1\n2\n3\n".to_vec(),
            b"1\nX\n3\n".to_vec(),
        ),
        (
            "theirs-emptied-ours-modified",
            b"1\nX\n3\n".to_vec(),
            b"1\n2\n3\n".to_vec(),
            empty.to_vec(),
        ),
        (
            "no-final-newline-both-sides",
            b"a\nb\nOURS".to_vec(),
            b"a\nb\n".to_vec(),
            b"a\nb\nTHEIRS".to_vec(),
        ),
        (
            "no-final-newline-one-side",
            b"a\nb\nOURS\n".to_vec(),
            b"a\nb\n".to_vec(),
            b"a\nb\nTHEIRS".to_vec(),
        ),
        (
            "no-final-newline-base-too",
            b"a\nb".to_vec(),
            b"a\nB".to_vec(),
            b"a\nZ".to_vec(),
        ),
        (
            "crlf-conflict",
            b"x\r\nOURS\r\nz\r\n".to_vec(),
            b"x\r\ny\r\nz\r\n".to_vec(),
            b"x\r\nTHEIRS\r\nz\r\n".to_vec(),
        ),
        (
            "crlf-clean-disjoint",
            b"x\r\nY\r\nz\r\n".to_vec(),
            b"x\r\nA\r\nB\r\nz\r\n".to_vec(),
            b"x\r\nA\r\nB\r\nZ\r\n".to_vec(),
        ),
        (
            "cjk-conflict",
            "第一行\n我们的修改\n第三行\n".as_bytes().to_vec(),
            "第一行\n原内容\n第三行\n".as_bytes().to_vec(),
            "第一行\n对方的修改\n第三行\n".as_bytes().to_vec(),
        ),
        (
            "cjk-clean",
            "第一行\n第二行\n我们的修改\n".as_bytes().to_vec(),
            "第一行\n第二行\n第三行\n".as_bytes().to_vec(),
            "对方的第一行\n第二行\n第三行\n".as_bytes().to_vec(),
        ),
        (
            "emoji-and-tabs",
            "\tA\n🌱 ours\n".as_bytes().to_vec(),
            "\tA\n🌱 base\n".as_bytes().to_vec(),
            "\tA\n🌱 theirs\n".as_bytes().to_vec(),
        ),
        (
            "many-hunks-many-conflicts",
            (0..30)
                .map(|i| format!("ours{i}\n"))
                .collect::<Vec<_>>()
                .join("")
                .into_bytes(),
            (0..30)
                .map(|i| format!("base{i}\n"))
                .collect::<Vec<_>>()
                .join("")
                .into_bytes(),
            (0..30)
                .map(|i| format!("theirs{i}\n"))
                .collect::<Vec<_>>()
                .join("")
                .into_bytes(),
        ),
    ];

    let mut conflicts = 0usize;
    let mut clean = 0usize;
    for (name, ours, base, theirs) in &cases {
        for diff3 in [false, true] {
            if engine_case(name, ours, base, theirs, diff3) {
                conflicts += 1;
            } else {
                clean += 1;
            }
        }
    }
    // 反退化：语料必须同时包含「干净」和「冲突」两类，否则断言可能恒真。
    assert!(
        clean >= 5,
        "corpus degenerated: only {clean} clean comparisons"
    );
    assert!(
        conflicts >= 5,
        "corpus degenerated: only {conflicts} conflict comparisons"
    );
}

#[test]
fn engine_identity_shortcuts_match_git() {
    let a = b"same\ncontent\n".to_vec();
    let b = b"other\ncontent\n".to_vec();
    for diff3 in [false, true] {
        assert_eq!(
            merge_blobs(Some(&a), &a, &a, &labels(diff3)).unwrap(),
            Merged::Clean(a.clone())
        );
        assert_eq!(
            merge_blobs(Some(&a), &a, &b, &labels(diff3)).unwrap(),
            Merged::Clean(b.clone())
        );
        assert_eq!(
            merge_blobs(Some(&b), &a, &b, &labels(diff3)).unwrap(),
            Merged::Clean(a.clone())
        );
        engine_case("identity-base==ours", &a, &a, &b, diff3);
        engine_case("identity-base==theirs", &a, &b, &a, diff3);
        engine_case("identity-ours==theirs", &a, &b, &b, diff3);
    }
}

#[test]
fn engine_none_base_two_added_files() {
    let same = b"identical\n".to_vec();
    let other = b"different\n".to_vec();
    for diff3 in [false, true] {
        assert_eq!(
            merge_blobs(None, &same, &same, &labels(diff3)).unwrap(),
            Merged::Clean(same.clone())
        );
        match merge_blobs(None, &same, &other, &labels(diff3)).unwrap() {
            Merged::Conflict(bytes) => assert!(!bytes.is_empty()),
            Merged::Clean(bytes) => panic!(
                "base=None 且两侧内容不同必须冲突，却返回 Clean({:?})",
                String::from_utf8_lossy(&bytes)
            ),
        }
        // base=None 应与「base 为空文件」等价，并与真实 git 对拍。
        engine_case("none-base-vs-empty-base", &same, b"", &other, diff3);
        assert_eq!(
            merge_blobs(None, &same, &other, &labels(diff3)).unwrap(),
            merge_blobs(Some(b""), &same, &other, &labels(diff3)).unwrap(),
            "base=None 与 base=empty 的结果应当一致"
        );
    }
}

#[test]
fn engine_binary_is_conflict_with_ours() {
    // 真值来源自证：`git merge-file` 对二进制输入直接报错拒绝（退出码 255），
    // 因此二进制语义的真值只能来自 `git merge`（见 CLI 场景 `conflict-binary`）。
    let ours = b"OURS\0BYTES\n".to_vec();
    let base = b"BASE\0BYTES\n".to_vec();
    let theirs = b"THEIRS\0BYTES\n".to_vec();
    let plain_ours = b"plain ours\n".to_vec();
    let plain_base = b"plain base\n".to_vec();
    let plain_theirs = b"plain theirs\n".to_vec();

    let dir = tempfile::tempdir().unwrap();
    write_file(&dir.path().join("t_ours"), &ours);
    write_file(&dir.path().join("t_base"), &base);
    write_file(&dir.path().join("t_theirs"), &theirs);
    let out = run(git_cmd(dir.path()).args([
        "merge-file",
        "-p",
        "-L",
        "ours",
        "-L",
        "base",
        "-L",
        "theirs",
        "t_ours",
        "t_base",
        "t_theirs",
    ]));
    assert_eq!(
        code(&out),
        255,
        "自证失败：git merge-file 对二进制输入的退出码应为 255，实际 {} ({})",
        code(&out),
        out_text(&out)
    );

    for diff3 in [false, true] {
        // 任一侧（或 base）二进制 → Conflict(ours)，且不插 marker。
        for (label, o, b, t) in [
            ("base-binary", &plain_ours, &base, &plain_theirs),
            ("ours-binary", &ours, &plain_base, &plain_theirs),
            ("theirs-binary", &plain_ours, &plain_base, &theirs),
        ] {
            match merge_blobs(Some(b), o, t, &labels(diff3)).unwrap() {
                Merged::Conflict(bytes) => assert_eq!(
                    &bytes, o,
                    "{label}: 二进制冲突必须原样保留 ours（不插 marker）"
                ),
                Merged::Clean(bytes) => panic!(
                    "{label}: 二进制必须判冲突，却返回 Clean({:?})",
                    String::from_utf8_lossy(&bytes)
                ),
            }
        }

        // 窗口边界：前 8000 字节内的 NUL 触发二进制短路。
        let mut early = b"aaa".to_vec();
        early.push(0);
        early.extend_from_slice(b"bbb\n");
        match merge_blobs(Some(b"x\n"), &early, b"y\n", &labels(diff3)).unwrap() {
            Merged::Conflict(bytes) => assert_eq!(bytes, early),
            Merged::Clean(_) => panic!("前 8000 字节含 NUL 必须判二进制冲突"),
        }

        // 8000 字节之外的 NUL 不触发短路：必须走行级合并（结果 != ours）。
        let mut late = vec![b'a'; 9000];
        late.push(0);
        late.push(b'\n');
        match merge_blobs(Some(b"x\n"), &late, b"y\n", &labels(diff3)).unwrap() {
            Merged::Conflict(bytes) => {
                assert_ne!(
                    bytes, late,
                    "8000 字节外的 NUL 不该走二进制短路（那会恒等返回 ours）"
                );
                assert!(
                    String::from_utf8_lossy(&bytes).contains("======="),
                    "非二进制路径应产生带 marker 的冲突输出"
                );
            }
            Merged::Clean(bytes) => panic!(
                "两侧都改同一行应当冲突，却返回 Clean({:?})",
                String::from_utf8_lossy(&bytes)
            ),
        }
    }
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 11
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
}

fn mutate_side(rng: &mut Lcg, base: &[String], indent: bool) -> Vec<u8> {
    let ops = 1 + rng.below(6);
    let mut lines = base.to_vec();
    for _ in 0..ops {
        if lines.is_empty() {
            lines.push("ins".to_string());
            continue;
        }
        let idx = rng.below(lines.len() as u64) as usize;
        match rng.below(3) {
            0 => lines[idx] = format!("M{}", rng.next() % 1000),
            1 => {
                lines.remove(idx);
            }
            _ => lines.insert(idx, format!("I{}", rng.next() % 1000)),
        }
    }
    if indent {
        lines = lines
            .iter()
            .map(|line| format!("{}{}", " ".repeat(rng.below(8) as usize), line))
            .collect();
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text.into_bytes()
}

#[test]
fn engine_random_corpus_matches_git() {
    let mut rng = Lcg(0x5eed_10a1_2026_0919);
    let mut clean = 0usize;
    let mut conflicts = 0usize;
    for case in 0..200 {
        let n = 3 + rng.below(20) as usize;
        let base: Vec<String> = (0..n).map(|i| format!("line{i}")).collect();
        let mut base_text = base.join("\n");
        base_text.push('\n');
        let ours = mutate_side(&mut rng, &base, false);
        let theirs = mutate_side(&mut rng, &base, false);
        for diff3 in [false, true] {
            let name = format!("random#{case}");
            if engine_case(&name, &ours, base_text.as_bytes(), &theirs, diff3) {
                conflicts += 1;
            } else {
                clean += 1;
            }
        }
    }
    // 反退化：语料必须同时覆盖两类结果。
    assert!(clean >= 20, "随机语料太偏：clean={clean}（断言可能恒真）");
    assert!(
        conflicts >= 20,
        "随机语料太偏：conflicts={conflicts}（断言可能恒真）"
    );
    println!("random corpus: clean={clean} conflicts={conflicts}");
}

/// 缩进语料是「已知差异」的压力面：`git merge-file` 传 `xpp.flags = 0`（关掉
/// 缩进启发式），而 T6 的 myers 统一启用缩进启发式（`git diff` 默认）。
/// 本用例**不**把它算进验收标准，只做特征化记录并把分叉数量打印出来，
/// 避免把已知限制混进 PASS/FAIL 结论。
#[test]
fn engine_indented_corpus_characterization() {
    let mut rng = Lcg(0x1234_5678_9abc_def0);
    let mut diverged = 0usize;
    let mut total = 0usize;
    for case in 0..120 {
        let n = 4 + rng.below(16) as usize;
        let base: Vec<String> = (0..n).map(|i| format!("body{i}")).collect();
        let mut base_text = base.join("\n");
        base_text.push('\n');
        let ours = mutate_side(&mut rng, &base, true);
        let theirs = mutate_side(&mut rng, &base, true);
        for diff3 in [false, true] {
            total += 1;
            let dir = tempfile::tempdir().unwrap();
            let Some((truth, _)) =
                truth_merge_file(dir.path(), &ours, base_text.as_bytes(), &theirs, diff3)
            else {
                continue;
            };
            let merged =
                merge_blobs(Some(base_text.as_bytes()), &ours, &theirs, &labels(diff3)).unwrap();
            let bytes = match merged {
                Merged::Clean(bytes) | Merged::Conflict(bytes) => bytes,
            };
            if bytes != truth {
                diverged += 1;
                if diverged <= 2 {
                    println!(
                        "indent divergence case#{case} diff3={diff3}\n mg ={:?}\n git={:?}",
                        String::from_utf8_lossy(&bytes),
                        String::from_utf8_lossy(&truth)
                    );
                }
            }
        }
    }
    println!("indented characterization: {diverged}/{total} differ from git merge-file");
}

// ============================================================== (B2) CLI 级

#[test]
fn cli_clean_merge_disjoint_files_and_tree_matches_git() {
    let p = pair(
        "clean-disjoint-files",
        &|repo| {
            history(
                repo,
                &[Ch::Set("a.txt", b"1\n2\n3\n"), Ch::Set("b.txt", b"x\n")],
                &[Ch::Set("a.txt", b"1\n2\n3\n4\n")],
                &[Ch::Set("b.txt", b"x\ny\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_clean();
}

#[test]
fn cli_clean_merge_same_file_different_regions() {
    let base: &[u8] = b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let p = pair(
        "clean-same-file-regions",
        &|repo| {
            history(
                repo,
                &[Ch::Set("f.txt", base)],
                &[Ch::Set("f.txt", b"1\n2\n3\n4\n5\n6\n7\n8\n9\nOURS\n")],
                &[Ch::Set("f.txt", b"THEIRS\n2\n3\n4\n5\n6\n7\n8\n9\n10\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_clean();
}

#[test]
fn cli_clean_merge_nested_dirs_and_new_files() {
    let p = pair(
        "clean-nested-dirs",
        &|repo| {
            history(
                repo,
                &[
                    Ch::Set("dir/deep/keep.txt", b"keep\n"),
                    Ch::Set("dir/other.txt", b"other\n"),
                ],
                &[Ch::Set("dir/deep/keep.txt", b"keep changed\n")],
                &[
                    Ch::Set("dir/other.txt", b"other changed\n"),
                    Ch::Set("dir/new/nested.txt", b"new\n"),
                ],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_clean();
}

#[test]
fn cli_clean_merge_ours_delete_theirs_untouched() {
    let p = pair(
        "clean-ours-delete",
        &|repo| {
            history(
                repo,
                &[
                    Ch::Set("gone.txt", b"gone\n"),
                    Ch::Set("stay.txt", b"stay\n"),
                ],
                &[Ch::Del("gone.txt")],
                &[Ch::Set("stay.txt", b"stay2\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_clean();
}

#[test]
fn cli_conflict_same_line() {
    let p = pair(
        "conflict-same-line",
        &|repo| {
            history(
                repo,
                &[Ch::Set("f.txt", b"1\n2\n3\n"), Ch::Set("g.txt", b"g\n")],
                &[
                    Ch::Set("f.txt", b"1\nOURS\n3\n"),
                    Ch::Set("clean.txt", b"added\n"),
                ],
                &[Ch::Set("f.txt", b"1\nTHEIRS\n3\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_conflict(&["f.txt"]);
}

#[test]
fn cli_conflict_add_add_same_path() {
    let p = pair(
        "conflict-add-add",
        &|repo| {
            history(
                repo,
                &[Ch::Set("keep.txt", b"keep\n")],
                &[Ch::Set("same.txt", b"ours version\n")],
                &[Ch::Set("same.txt", b"theirs version\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_conflict(&["same.txt"]);
}

#[test]
fn cli_conflict_delete_modify_both_directions() {
    // du.txt：ours 删、theirs 改 → DU；ud.txt：ours 改、theirs 删 → UD。
    let p = pair(
        "conflict-delete-modify",
        &|repo| {
            history(
                repo,
                &[
                    Ch::Set("du.txt", b"a\nb\nc\n"),
                    Ch::Set("ud.txt", b"a\nb\nc\n"),
                ],
                &[Ch::Del("du.txt"), Ch::Set("ud.txt", b"a\nOURS\nc\n")],
                &[Ch::Set("du.txt", b"a\nTHEIRS\nc\n"), Ch::Del("ud.txt")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    // 冲突码必须在 assert_conflict（会解决冲突并提交）之前取。
    let porcelain = p.porcelain_mg();
    assert!(
        porcelain.contains("DU ") && porcelain.contains("UD "),
        "{}: 期望同时出现 DU 与 UD，实际 {porcelain:?}",
        p.tag
    );
    p.assert_conflict(&["du.txt", "ud.txt"]);
}

#[test]
fn cli_clean_both_sides_delete_same_file() {
    let p = pair(
        "clean-both-delete",
        &|repo| {
            history(
                repo,
                &[
                    Ch::Set("gone.txt", b"gone\n"),
                    Ch::Set("stay.txt", b"stay\n"),
                ],
                &[Ch::Del("gone.txt")],
                &[Ch::Del("gone.txt"), Ch::Set("added.txt", b"new\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_clean();
    assert!(
        !p.mg_repo.join("gone.txt").exists(),
        "{}: 两侧都删除的文件必须保持删除",
        p.tag
    );
}

#[test]
fn cli_conflict_modified_then_deleted_by_theirs() {
    let p = pair(
        "conflict-theirs-delete-ours-modify",
        &|repo| {
            history(
                repo,
                &[Ch::Set("f.txt", b"a\nb\nc\n"), Ch::Set("k.txt", b"k\n")],
                &[Ch::Set("f.txt", b"a\nOURS\nc\n")],
                &[Ch::Del("f.txt"), Ch::Set("k.txt", b"k2\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    assert!(!p.mg_out.status.success(), "{}: must conflict", p.tag);
    assert!(!p.git_out.status.success(), "{}: git oracle", p.tag);
    assert_eq!(
        p.porcelain_mg(),
        p.porcelain_git(),
        "{}: UD/DM conflict codes differ",
        p.tag
    );
    assert_eq!(
        p.stage_mg(),
        p.stage_git(),
        "{}: index stages differ",
        p.tag
    );
    assert_eq!(snapshot(&p.mg_repo), snapshot(&p.git_repo));
    p.fsck(&p.mg_repo);
}

#[test]
fn cli_conflict_crlf_bytes_match_git() {
    let p = pair(
        "conflict-crlf",
        &|repo| {
            history(
                repo,
                &[Ch::Set("f.txt", b"x\r\ny\r\nz\r\n")],
                &[Ch::Set("f.txt", b"x\r\nOURS\r\nz\r\n")],
                &[Ch::Set("f.txt", b"x\r\nTHEIRS\r\nz\r\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_conflict(&["f.txt"]);
}

#[test]
fn cli_conflict_no_trailing_newline_bytes_match_git() {
    let p = pair(
        "conflict-no-trailing-newline",
        &|repo| {
            history(
                repo,
                &[Ch::Set("f.txt", b"a\nb\nc")],
                &[Ch::Set("f.txt", b"a\nOURS\nc\n")],
                &[Ch::Set("f.txt", b"a\nTHEIRS\nc")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_conflict(&["f.txt"]);
}

#[test]
fn cli_conflict_cjk_bytes_match_git() {
    let p = pair(
        "conflict-cjk",
        &|repo| {
            history(
                repo,
                &[Ch::Set("文件.txt", "第一行\n原内容\n第三行\n".as_bytes())],
                &[Ch::Set(
                    "文件.txt",
                    "第一行\n我们的修改\n第三行\n".as_bytes(),
                )],
                &[Ch::Set(
                    "文件.txt",
                    "第一行\n对方的修改\n第三行\n".as_bytes(),
                )],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_conflict(&["文件.txt"]);
}

#[test]
fn cli_conflict_binary_keeps_ours_like_git() {
    let ours: &[u8] = b"OURS\0binary\npayload\n";
    let theirs: &[u8] = b"THEIRS\nbinary\npayload\n";
    let base: &[u8] = b"BASE\0binary\npayload\n";
    // 三种「哪一侧是二进制」的组合，逐条与真实 git merge 的产物对拍：
    //   blob.bin        两侧都是二进制（base 也是）
    //   blob_ours.bin   只有 ours 是二进制
    //   blob_theirs.bin 只有 theirs 是二进制
    let ours_only: &[u8] = b"OURS\0only-ours\n";
    let theirs_only: &[u8] = b"THEIRS\0only-theirs\n";
    let textual_base: &[u8] = b"textual base\n";
    let p = pair(
        "conflict-binary",
        &|repo| {
            history(
                repo,
                &[
                    Ch::Set("blob.bin", base),
                    Ch::Set("blob_ours.bin", textual_base),
                    Ch::Set("blob_theirs.bin", textual_base),
                ],
                &[
                    Ch::Set("blob.bin", ours),
                    Ch::Set("blob_ours.bin", ours_only),
                    Ch::Set("blob_theirs.bin", b"textual ours\n"),
                ],
                &[
                    Ch::Set("blob.bin", theirs),
                    Ch::Set("blob_ours.bin", b"textual theirs\n"),
                    Ch::Set("blob_theirs.bin", theirs_only),
                ],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    // 冲突现场字节（assert_conflict 会把它解决成确定内容，所以先取）。
    let expected: [(&str, &[u8]); 3] = [
        ("blob.bin", ours),
        ("blob_ours.bin", ours_only),
        ("blob_theirs.bin", b"textual ours\n"),
    ];
    for (path, want) in expected {
        let mg_bytes = fs::read(p.mg_repo.join(path)).unwrap();
        let git_bytes = fs::read(p.git_repo.join(path)).unwrap();
        assert_eq!(
            &git_bytes, want,
            "真值自证：真实 git merge 在 {path} 的冲突现场应保留 ours 原样"
        );
        assert_eq!(
            mg_bytes, want,
            "{path}: 二进制冲突必须原样保留 ours（不插 marker）"
        );
    }
    p.assert_conflict(&["blob.bin", "blob_ours.bin", "blob_theirs.bin"]);
}

#[test]
fn cli_conflict_mixed_clean_and_conflicting_paths() {
    let p = pair(
        "conflict-mixed",
        &|repo| {
            history(
                repo,
                &[
                    Ch::Set("conf1.txt", b"1\n2\n3\n"),
                    Ch::Set("conf2.txt", b"a\nb\nc\n"),
                    Ch::Set("clean.txt", b"clean base\n"),
                ],
                &[
                    Ch::Set("conf1.txt", b"1\nOURS\n3\n"),
                    Ch::Set("conf2.txt", b"a\nOURS\nc\n"),
                    Ch::Set("clean.txt", b"clean base\nours addition\n"),
                ],
                &[
                    Ch::Set("conf1.txt", b"1\nTHEIRS\n3\n"),
                    Ch::Set("conf2.txt", b"a\nTHEIRS\nc\n"),
                    Ch::Set("added-by-theirs.txt", b"new\n"),
                ],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    p.assert_conflict(&["conf1.txt", "conf2.txt"]);
    assert_eq!(
        fs::read(p.mg_repo.join("clean.txt")).unwrap(),
        b"clean base\nours addition\n",
        "非冲突路径必须被正确合并写入工作区"
    );
}

#[test]
fn cli_conflict_diff3_style_matches_git() {
    let p = pair(
        "conflict-diff3",
        &|repo| {
            // mg 通过 Config::get("merge.conflictstyle") 读仓库本地配置。
            git_ok(repo, &["config", "merge.conflictstyle", "diff3"]);
            history(
                repo,
                &[Ch::Set("f.txt", b"1\n2\n3\n")],
                &[Ch::Set("f.txt", b"1\nOURS\n3\n")],
                &[Ch::Set("f.txt", b"1\nTHEIRS\n3\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    let mg_bytes = fs::read(p.mg_repo.join("f.txt")).unwrap();
    let text = String::from_utf8_lossy(&mg_bytes).to_string();
    assert!(
        text.contains("|||||||"),
        "{}: diff3 风格必须输出 `||||||| <base>` 段，实际 {:?}",
        p.tag,
        text
    );
    assert!(
        String::from_utf8_lossy(&fs::read(p.git_repo.join("f.txt")).unwrap()).contains("|||||||"),
        "{}: 真值 git 也应是 diff3 风格（来源自证）",
        p.tag
    );
    p.assert_conflict(&["f.txt"]);
}

#[test]
fn cli_untracked_file_that_theirs_adds_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    new_repo(&repo);
    history(
        &repo,
        &[Ch::Set("f.txt", b"base\n")],
        &[Ch::Set("f.txt", b"ours\n")],
        &[Ch::Set("new.txt", b"from theirs\n")],
    );
    // 未跟踪文件与 theirs 新增的同名 → 真实 git 也会拒绝覆盖。
    write_file(&repo.join("new.txt"), b"local untracked\n");
    let out = mg(&repo, &["merge", "feature"]);
    let text = out_text(&out);
    assert!(
        !out.status.success(),
        "未跟踪文件将被覆盖时必须拒绝：{text}"
    );
    assert!(!text.contains("panicked"), "不许 panic：{text}");
    assert_eq!(
        fs::read(repo.join("new.txt")).unwrap(),
        b"local untracked\n",
        "未跟踪文件必须原样保留"
    );
    assert!(
        text.contains("untracked") || text.contains("overwritten"),
        "应给出清晰信息，实际 {text}"
    );
    // 真值自证：同一状态的真实 git 也拒绝。
    let root2 = tempfile::tempdir().unwrap();
    let repo2 = root2.path().join("repo");
    new_repo(&repo2);
    history(
        &repo2,
        &[Ch::Set("f.txt", b"base\n")],
        &[Ch::Set("f.txt", b"ours\n")],
        &[Ch::Set("new.txt", b"from theirs\n")],
    );
    write_file(&repo2.join("new.txt"), b"local untracked\n");
    let oracle = git(&repo2, &["merge", "--no-edit", "feature"]);
    assert!(
        !oracle.status.success(),
        "真值自证失败：真实 git 应当也拒绝未跟踪文件被覆盖，实际 {}",
        out_text(&oracle)
    );
}

/// 特征化记录：`merge.conflictstyle = zdiff3`。
///
/// 任务书只要求 diff3；但作者自述「支持 diff3/zdiff3」。实测：mg 把 zdiff3 也当
/// diff3 处理（`src/cli/merge.rs` 里 `diff3 || zdiff3` 走同一个 `LEVEL_EAGER`），
/// 而真实 git 的 zdiff3 会把冲突块里**两侧共有的前缀/后缀行**挪到 marker 之外，
/// 因此两者在「冲突块内含有公共行」的输入上并不逐字节相同。本用例只做记录，
/// 不当作验收标准（PASS/FAIL 依据仍在 diff3 的逐字节对拍上）。
#[test]
fn cli_conflictstyle_zdiff3_characterization() {
    let p = pair(
        "conflictstyle-zdiff3",
        &|repo| {
            git_ok(repo, &["config", "merge.conflictstyle", "zdiff3"]);
            history(
                repo,
                &[Ch::Set("f.txt", b"1\n2\n3\n4\n5\n6\n")],
                &[Ch::Set("f.txt", b"1\n2\nT-shared\nO-only\n6\n")],
                &[Ch::Set("f.txt", b"1\n2\nT-shared\nT-only\n6\n")],
            )
        },
        &["merge", "feature"],
        &["merge", "--no-edit", "feature"],
    );
    assert!(!p.mg_out.status.success() && !p.git_out.status.success());
    let mg_bytes = fs::read(p.mg_repo.join("f.txt")).unwrap();
    let git_bytes = fs::read(p.git_repo.join("f.txt")).unwrap();
    let mg_text = String::from_utf8_lossy(&mg_bytes).to_string();
    let git_text = String::from_utf8_lossy(&git_bytes).to_string();
    assert!(mg_text.contains("|||||||") && git_text.contains("|||||||"));
    let same = mg_bytes == git_bytes;
    println!("zdiff3_matches_git={same}");
    if !same {
        println!("  mg :{:?}", mg_text);
        println!("  git:{:?}", git_text);
        println!("  结论：mg 把 zdiff3 当 diff3；真实 git 的 zdiff3 把公共行挪出 marker 之外（已知限制，任务书未要求）");
    }
}

#[test]
fn cli_fast_forward_matches_git() {
    let p = pair(
        "fast-forward",
        &|repo| {
            write_file(&repo.join("f.txt"), b"a\n");
            git_ok(repo, &["add", "-A"]);
            git_ok(repo, &["commit", "-qm", "base"]);
            git_ok(repo, &["checkout", "-qb", "feature"]);
            write_file(&repo.join("f.txt"), b"a\nb\n");
            git_ok(repo, &["add", "-A"]);
            git_ok(repo, &["commit", "-qm", "theirs"]);
            git_ok(repo, &["checkout", "-q", "main"]);
        },
        &["merge", "feature"],
        &["merge", "feature"],
    );
    assert!(
        p.mg_out.status.success(),
        "{}: {}",
        p.tag,
        out_text(&p.mg_out)
    );
    assert!(
        String::from_utf8_lossy(&p.mg_out.stdout).contains("Fast-forward"),
        "{}: fast-forward 应打印 Fast-forward，实际 {:?}",
        p.tag,
        String::from_utf8_lossy(&p.mg_out.stdout)
    );
    assert_eq!(p.tree_mg(), p.tree_git(), "{}: ff tree differs", p.tag);
    assert_eq!(p.head_mg(), p.head_git(), "{}: ff HEAD oid differs", p.tag);
    assert_eq!(p.porcelain_mg(), "", "{}: worktree must be clean", p.tag);
    assert_eq!(p.porcelain_mg(), p.porcelain_git());
    assert_eq!(p.stage_mg(), p.stage_git());
    assert_eq!(
        p.parents(&p.mg_repo).len(),
        1,
        "{}: fast-forward 不应产生合并提交",
        p.tag
    );
    p.fsck(&p.mg_repo);
}

#[test]
fn cli_no_ff_creates_two_parent_merge_commit_like_git() {
    let p = pair(
        "no-ff",
        &|repo| {
            write_file(&repo.join("f.txt"), b"a\n");
            git_ok(repo, &["add", "-A"]);
            git_ok(repo, &["commit", "-qm", "base"]);
            git_ok(repo, &["checkout", "-qb", "feature"]);
            write_file(&repo.join("f.txt"), b"a\nb\n");
            git_ok(repo, &["add", "-A"]);
            git_ok(repo, &["commit", "-qm", "theirs"]);
            git_ok(repo, &["checkout", "-q", "main"]);
        },
        &["merge", "--no-ff", "feature"],
        &["merge", "--no-ff", "--no-edit", "feature"],
    );
    p.assert_clean();
    assert_eq!(
        p.parents(&p.mg_repo).len(),
        2,
        "{}: --no-ff 必须产生两父合并提交",
        p.tag
    );
}

#[test]
fn cli_already_up_to_date_when_target_is_ancestor() {
    let p = pair(
        "already-up-to-date",
        &|repo| {
            write_file(&repo.join("f.txt"), b"a\n");
            git_ok(repo, &["add", "-A"]);
            git_ok(repo, &["commit", "-qm", "base"]);
            git_ok(repo, &["branch", "feature"]);
            write_file(&repo.join("f.txt"), b"a\nb\n");
            git_ok(repo, &["add", "-A"]);
            git_ok(repo, &["commit", "-qm", "ours"]);
        },
        &["merge", "feature"],
        &["merge", "feature"],
    );
    assert!(
        p.mg_out.status.success(),
        "{}: {}",
        p.tag,
        out_text(&p.mg_out)
    );
    let text = String::from_utf8_lossy(&p.mg_out.stdout).to_string();
    assert!(
        text.contains("Already up to date"),
        "{}: stdout 必须包含 `Already up to date`，实际 {text:?}",
        p.tag
    );
    assert!(
        String::from_utf8_lossy(&p.git_out.stdout).contains("Already up to date"),
        "{}: 真值 git 也包含该字样（来源自证）",
        p.tag
    );
    assert_eq!(p.head_mg(), p.head_git(), "{}: HEAD 不应移动", p.tag);
    assert_eq!(p.porcelain_mg(), "");
    assert_eq!(snapshot(&p.mg_repo), snapshot(&p.git_repo));
}

#[test]
fn cli_dirty_worktree_is_refused_with_zero_changes() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    new_repo(&repo);
    history(
        &repo,
        &[Ch::Set("f.txt", b"1\n2\n3\n")],
        &[Ch::Set("f.txt", b"1\nOURS\n3\n")],
        &[Ch::Set("f.txt", b"1\nTHEIRS\n3\n")],
    );
    write_file(&repo.join("f.txt"), b"1\nDIRTY LOCAL\n3\n");
    let before = sha_manifest(&repo);
    let head_before = git_ok(&repo, &["rev-parse", "HEAD"]);
    let out = mg(&repo, &["merge", "feature"]);
    let after = sha_manifest(&repo);
    assert!(
        !out.status.success(),
        "工作区脏时必须拒绝合并，实际退出码 0：{}",
        out_text(&out)
    );
    assert!(
        !out_text(&out).contains("panicked"),
        "不许 panic：{}",
        out_text(&out)
    );
    assert_eq!(
        before, after,
        "拒绝合并时不得改动任何文件（sha256sum 前后必须一致）"
    );
    assert_eq!(head_before, git_ok(&repo, &["rev-parse", "HEAD"]));
    assert_eq!(
        fs::read(repo.join("f.txt")).unwrap(),
        b"1\nDIRTY LOCAL\n3\n",
        "本地修改必须原样保留"
    );
    assert!(
        out_text(&out).contains("refusing to overwrite local changes"),
        "应报 WouldLoseChanges，实际 {}",
        out_text(&out)
    );
}

#[test]
fn cli_unknown_rev_and_unknown_oid_fail_without_panic() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    new_repo(&repo);
    history(
        &repo,
        &[Ch::Set("f.txt", b"1\n")],
        &[Ch::Set("f.txt", b"2\n")],
        &[Ch::Set("f.txt", b"3\n")],
    );
    let before = sha_manifest(&repo);

    let out = mg(&repo, &["merge", "no-such-branch"]);
    assert!(!out.status.success(), "不存在的 rev 必须报错");
    let text = out_text(&out);
    assert!(!text.contains("panicked"), "不许 panic：{text}");
    assert!(
        text.contains("reference not found"),
        "应为 RefNotFound（reference not found），实际 {text}"
    );

    let out = mg(
        &repo,
        &["merge", "0123456789012345678901234567890123456789"],
    );
    assert!(!out.status.success(), "不存在的 oid 必须报错");
    let text = out_text(&out);
    assert!(!text.contains("panicked"), "不许 panic：{text}");
    assert!(
        text.contains("object not found"),
        "应为 ObjectNotFound（object not found），实际 {text}"
    );

    assert_eq!(before, sha_manifest(&repo), "报错路径不得改动仓库");
}

#[test]
fn cli_unrelated_histories_are_refused_loudly() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    new_repo(&repo);
    write_file(&repo.join("f.txt"), b"main\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["commit", "-qm", "main root"]);
    git_ok(&repo, &["checkout", "-q", "--orphan", "feature"]);
    git_ok(&repo, &["rm", "-q", "-r", "--cached", "."]);
    write_file(&repo.join("g.txt"), b"feature\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["commit", "-qm", "feature root"]);
    git_ok(&repo, &["checkout", "-q", "main"]);

    let out = mg(&repo, &["merge", "feature"]);
    let text = out_text(&out);
    assert!(!out.status.success(), "无共同祖先必须报错而不是静默");
    assert!(!text.contains("panicked"), "不许 panic：{text}");
    assert!(
        text.contains("unsupported") || text.contains("unrelated"),
        "应有清晰的错误信息（任务书允许 Unsupported），实际 {text}"
    );
}

// ==================================================== 真值来源自证（§4）

#[test]
fn truth_source_selfcheck_merge_file_exit_code_is_conflict_count() {
    let dir = tempfile::tempdir().unwrap();
    let (_, exit) = truth_merge_file(
        dir.path(),
        b"1\nOURS\n3\n4\n5\n",
        b"1\n2\n3\n4\n5\n",
        b"1\n2\n3\n4\nTHEIRS\n",
        false,
    )
    .expect("git can merge");
    assert_eq!(exit, 0, "无冲突时 git merge-file 退出码应为 0");
    let (bytes, exit) = truth_merge_file(
        dir.path(),
        b"1\nA\n3\n4\n5\n6\n7\n8\n9\nB\n10\n",
        b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n",
        b"1\nX\n3\n4\n5\n6\n7\n8\n9\nY\n10\n",
        false,
    )
    .expect("git can merge");
    assert_eq!(exit, 2, "两个（相隔较远的）冲突时退出码应为 2，实际 {exit}");
    assert_eq!(
        String::from_utf8_lossy(&bytes)
            .matches("<<<<<<< ours")
            .count(),
        2,
        "自证失败：退出码与 marker 数量应一致"
    );
}

#[test]
fn truth_source_selfcheck_porcelain_is_not_globally_sorted() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    new_repo(&repo);
    write_file(&repo.join("m.txt"), b"one\n");
    git_ok(&repo, &["add", "-A"]);
    git_ok(&repo, &["commit", "-qm", "base"]);
    write_file(&repo.join("m.txt"), b"two\n"); //  ' M m.txt'
    write_file(&repo.join("zzz_new.txt"), b"new\n");
    git_ok(&repo, &["add", "zzz_new.txt"]); //  'A  zzz_new.txt'
    write_file(&repo.join("aaa_untracked.txt"), b"a\n"); // '?? aaa_untracked.txt'
    let porcelain = git_ok(&repo, &["status", "--porcelain"]);
    let lines: Vec<String> = porcelain.lines().map(str::to_string).collect();
    assert!(
        lines.iter().any(|l| l.starts_with("A  "))
            && lines.iter().any(|l| l.starts_with("?? "))
            && lines.iter().any(|l| l.starts_with(" M ")),
        "porcelain 应同时含 staged/changed/untracked：{porcelain:?}"
    );
    let mut sorted = lines.clone();
    sorted.sort();
    assert_ne!(
        lines, sorted,
        "自证失败：本机 git status --porcelain 恰好是全局有序的；\
         分组顺序（已改动组 → 未跟踪组）不是全局字节序，所以对拍只能用原始输出"
    );
    let again = git_ok(&repo, &["status", "--porcelain"]);
    assert_eq!(porcelain, again, "同一状态下 porcelain 输出必须确定");
}

#[test]
fn truth_source_selfcheck_diff3_label_really_appears() {
    let dir = tempfile::tempdir().unwrap();
    let (plain, _) =
        truth_merge_file(dir.path(), b"1\nA\n3\n", b"1\n2\n3\n", b"1\nX\n3\n", false).unwrap();
    let (diff3, _) =
        truth_merge_file(dir.path(), b"1\nA\n3\n", b"1\n2\n3\n", b"1\nX\n3\n", true).unwrap();
    assert!(String::from_utf8_lossy(&plain).contains("======="));
    assert!(
        String::from_utf8_lossy(&diff3).contains("||||||| base"),
        "自证失败：--diff3 未生效，diff3 断言会退化为恒真"
    );
    assert_ne!(plain, diff3);
}
