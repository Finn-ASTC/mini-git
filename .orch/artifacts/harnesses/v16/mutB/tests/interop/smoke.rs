//! W0 冒烟测试：验证「测试床骨架」本身可用 ——
//! 工程能编译、mg 能跑、mg 建的仓库真实 git 认、hash-object 与 git 逐字节一致。
//!
//! 这些用例在 W0 结束时就该全绿；后续每个 wave 的验收都建立在它们之上。

use crate::common::{self, Scratch};

#[test]
fn mg_reports_version_and_help() {
    let scratch = Scratch::new();
    let out = scratch.mg(&["--version"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.starts_with("mg "),
        "--version should print 'mg <version>', got {text:?}"
    );

    let help = scratch.mg_ok(&["--help"]);
    for expected in [
        "hash-object",
        "cat-file",
        "status",
        "commit",
        "merge",
        "clone",
    ] {
        assert!(help.contains(expected), "--help should mention {expected}");
    }
}

#[test]
fn mg_init_produces_a_repository_real_git_accepts() {
    let scratch = Scratch::new();
    let stdout = scratch.init_with_mg();
    assert!(stdout.contains("Initialized empty"), "got {stdout:?}");

    assert_eq!(scratch.git_ok(&["rev-parse", "--git-dir"]), ".git");
    assert_eq!(scratch.git_ok(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(scratch.git_ok(&["status", "--porcelain"]), "");
    // 空仓库：fsck 应当无报错
    scratch.git_ok(&["fsck", "--no-progress"]);
    // 还没有提交
    let (ok, _, _) = scratch.git_raw(&["rev-parse", "--verify", "HEAD"]);
    assert!(!ok, "fresh repository should not have HEAD yet");
}

#[test]
fn real_git_can_commit_inside_an_mg_init_repository() {
    let scratch = Scratch::new();
    scratch.init_with_mg();
    scratch.write("a.txt", "hello\n");
    let head = scratch.git_commit_all("first");
    assert_eq!(head.len(), 40);
    assert_eq!(scratch.git_ok(&["log", "--oneline"]).lines().count(), 1);
    scratch.git_ok(&["fsck", "--no-progress"]);
}

#[test]
fn mg_can_discover_a_repository_created_by_real_git() {
    let scratch = Scratch::new();
    scratch.init_with_git();
    let nested = scratch.join("deep/nested");
    std::fs::create_dir_all(&nested).unwrap();

    let repo = minigit::Repo::discover(&nested).expect("mg should discover a git-created repo");
    assert_eq!(repo.git_dir(), scratch.join(".git").as_path());
    // mg 也要认「mg init 出来的仓库」
    let other = Scratch::new();
    other.init_with_mg();
    assert!(minigit::Repo::discover(other.path()).is_ok());
}

#[test]
fn hash_object_matches_real_git_byte_for_byte() {
    let scratch = Scratch::new();
    let cases: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"hello world\n".to_vec(),
        b"no trailing newline".to_vec(),
        "中文 + emoji 🚀\n".as_bytes().to_vec(),
        vec![0u8, 1, 2, 255, 254],
        vec![b'x'; 100_000],
    ];

    for (idx, payload) in cases.iter().enumerate() {
        let file = format!("case-{idx}.bin");
        scratch.write_bytes(&file, payload);

        let mg_oid = scratch.mg_ok(&["hash-object", &file]);
        let git_oid = scratch.git_ok(&["hash-object", &file]);
        common::assert_same(&format!("hash-object {file}"), &mg_oid, &git_oid);
    }
}

#[test]
fn hash_object_stdin_matches_real_git() {
    use std::io::Write;
    use std::process::Stdio;

    let scratch = Scratch::new();
    let payload = b"hello world\n";

    let mut mg = common::mg_command(scratch.path(), &["hash-object", "--stdin"]);
    mg.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = mg.spawn().unwrap();
    child.stdin.as_mut().unwrap().write_all(payload).unwrap();
    let mg_out = child.wait_with_output().unwrap();
    common::assert_success("mg hash-object --stdin", &mg_out);

    let mut git = common::git_command(scratch.path(), &["hash-object", "--stdin"]);
    git.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = git.spawn().unwrap();
    child.stdin.as_mut().unwrap().write_all(payload).unwrap();
    let git_out = child.wait_with_output().unwrap();
    common::assert_success("git hash-object --stdin", &git_out);

    common::assert_same(
        "hash-object --stdin",
        &common::trim(&mg_out.stdout),
        &common::trim(&git_out.stdout),
    );
}

/// W0 版本断言「`mg status` 必须报 not implemented」——那在命令实现之后就不再成立
/// （controller round，见 ORCHESTRATION.md C-18）。改成两条**长期有效**的契约：
/// 已实现的命令必须像 git 一样工作；非法用法必须响亮失败，而不是假装成功。
#[test]
fn implemented_commands_work_and_bad_usage_fails_loudly() {
    let scratch = Scratch::new();
    scratch.init_with_mg();

    let (ok, stdout, stderr) = scratch.mg_raw(&["status", "--porcelain"]);
    assert!(
        ok,
        "status is implemented; it must exit 0 on a fresh repo, stderr={stderr:?}"
    );
    assert!(
        stdout.trim().is_empty(),
        "a fresh repo must have empty porcelain output, got {stdout:?}"
    );

    let (ok, _, stderr) = scratch.mg_raw(&["definitely-not-a-command"]);
    assert!(!ok, "an unknown subcommand must not exit 0");
    assert!(
        !stderr.trim().is_empty(),
        "an unknown subcommand must explain itself on stderr"
    );
}
