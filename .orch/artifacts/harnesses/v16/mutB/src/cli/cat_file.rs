//! `mg cat-file` —— **T5（hermes）**：loose 对象；**T9 之后**：pack 回退。
//! rev 解析（`HEAD`、短名、`refs/...`）由 **T2** 的 `RefStore::resolve` 提供。
//!
//! 验收：`git cat-file -p <oid>` 与 `mg cat-file -p <oid>` 输出一致（tree 也要打印正确）。
//!
//! tree 的**显示格式跟着真实 git**（`git ls-tree` / `git cat-file -p <tree>` 都用 `%06o`，
//! 子树显示成 `040000`）。注意别与 tree **载荷**里的 mode 混淆：载荷写的是 5 位 `40000`
//! （`FileMode::as_str`），显示是 6 位。

use std::io::Write;

use crate::error::{Error, Result};
use crate::object::{Kind, Tree};
use crate::odb::Odb;
use crate::refs::RefStore;

pub fn run(
    show_type: bool,
    show_size: bool,
    pretty: bool,
    exists: bool,
    object: &str,
) -> Result<()> {
    let repo = crate::cli::open_repo()?;
    let store = RefStore::new(&repo);
    let oid = store.resolve(object)?;
    let odb = Odb::new(&repo);

    // `-e`：只回答「在不在」，用退出码表达，不打印任何东西。
    if exists {
        if odb.exists(oid) {
            return Ok(());
        }
        return Err(Error::ObjectNotFound(oid));
    }

    if !(show_type || show_size || pretty) {
        // 与真实 git 一致：不给选择器是用法错误，而不是「默认 pretty」。
        return Err(Error::Other(
            "usage: mg cat-file (-e | -p | -t | -s) <object>".to_string(),
        ));
    }

    let (kind, payload) = odb.read(oid)?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if show_type {
        writeln!(out, "{kind}")?;
    } else if show_size {
        writeln!(out, "{}", payload.len())?;
    } else {
        match kind {
            // tree 的 pretty 形态是 `git ls-tree` 的行格式，不是原始载荷。
            Kind::Tree => write_tree(&mut out, &Tree::decode_payload(&payload)?)?,
            // blob / commit / tag 的 pretty 形态就是载荷原文（逐字节，不加换行）。
            _ => out.write_all(&payload)?,
        }
    }
    out.flush()?;
    Ok(())
}

/// `<mode> <type> <oid>\t<name>\n`，逐字节与真实 git 相同。
fn write_tree(out: &mut impl Write, tree: &Tree) -> Result<()> {
    for entry in tree.entries() {
        write!(out, "{:06o} ", entry.mode.to_u32())?;
        out.write_all(if entry.mode.is_tree() {
            b"tree"
        } else {
            b"blob"
        })?;
        write!(out, " {}\t", entry.oid.to_hex())?;
        // 名字可能是非 UTF-8：按字节写，别做 lossy 转换。
        out.write_all(&entry.name)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};

    use super::*;
    use crate::oid::Oid;
    use crate::repo::{Repo, DEFAULT_INITIAL_BRANCH};

    // -------------------------------------------------------------- 测试床

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// 真值只能来自 git 二进制；配置/身份/日期就地隔离（不 export 进共享 shell）。
    fn git_raw(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Output {
        let mut cmd = Command::new("git");
        cmd.current_dir(dir)
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
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match stdin {
            Some(bytes) => {
                cmd.stdin(Stdio::piped());
                let mut child = cmd.spawn().expect("failed to spawn git");
                {
                    let mut sink = child.stdin.take().expect("git stdin");
                    sink.write_all(bytes).expect("write git stdin");
                }
                child.wait_with_output().expect("git output")
            }
            None => {
                cmd.stdin(Stdio::null());
                cmd.output().expect("failed to spawn git")
            }
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = git_raw(dir, args, None);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn git_out(dir: &Path, args: &[&str]) -> Output {
        git_raw(dir, args, None)
    }

    /// `mg` 二进制：优先用 cargo 给测试注入的路径，退化时从 `target/<profile>/deps/` 推。
    fn mg_binary() -> PathBuf {
        if let Some(path) = option_env!("CARGO_BIN_EXE_mg") {
            return PathBuf::from(path);
        }
        let exe = std::env::current_exe().expect("current_exe");
        let profile_dir = exe
            .parent()
            .and_then(Path::parent)
            .expect("target/<profile> dir");
        profile_dir.join(format!("mg{}", std::env::consts::EXE_SUFFIX))
    }

    /// 跑 mg；`GIT_*` 环境变量一律清掉（共享 shell 里可能被他人的实验污染）。
    fn mg_raw(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Output {
        let binary = mg_binary();
        assert!(
            binary.is_file(),
            "mg binary not found at {}: 先构建 bin target",
            binary.display()
        );
        let mut cmd = Command::new(&binary);
        cmd.current_dir(dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_OBJECT_DIRECTORY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match stdin {
            Some(bytes) => {
                cmd.stdin(Stdio::piped());
                let mut child = cmd.spawn().expect("failed to spawn mg");
                {
                    let mut sink = child.stdin.take().expect("mg stdin");
                    sink.write_all(bytes).expect("write mg stdin");
                }
                child.wait_with_output().expect("mg output")
            }
            None => {
                cmd.stdin(Stdio::null());
                cmd.output().expect("failed to spawn mg")
            }
        }
    }

    fn ok(out: Output, what: &str) -> Output {
        assert!(
            out.status.success(),
            "{what} failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn oid_at(dir: &Path, rev: &str) -> String {
        git(dir, &["rev-parse", rev])
    }

    /// 真实 git 仓库：含 blobs、子树（`foo.txt` 与 `foo/` 的排序陷阱）、可执行位、symlink、commit、annotated tag。
    fn init_git_repo(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "hello from git\n").unwrap();
        std::fs::write(dir.join("foo.txt"), "top level\n").unwrap();
        std::fs::create_dir_all(dir.join("foo")).unwrap();
        std::fs::write(dir.join("foo/b.txt"), "nested\n").unwrap();
        std::fs::write(dir.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("run.sh"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            std::os::unix::fs::symlink("a.txt", dir.join("link.txt")).unwrap();
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "initial"]);
        git(dir, &["tag", "-a", "v1", "-m", "annotated"]);
    }

    // ------------------------------------------- A. mg 自己写、mg 自己读

    /// 端到端：`mg hash-object -w` 写 blob → `mg cat-file -p/-t/-s` 逐字节还原（含 NUL 与非 UTF-8）。
    #[test]
    fn cat_file_roundtrips_a_blob_mg_wrote() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        Repo::init(dir, DEFAULT_INITIAL_BRANCH).unwrap();

        let payload = b"line one\nline two\n\x00binary\xff tail no newline".as_slice();
        let written = mg_raw(dir, &["hash-object", "-w", "--stdin"], Some(payload));
        let written = ok(written, "mg hash-object -w");
        assert!(written.stderr.is_empty(), "hash-object was noisy");
        let hex = String::from_utf8_lossy(&written.stdout).trim().to_string();
        assert_eq!(hex, Oid::hash_object("blob", payload).to_hex());

        let pretty = ok(
            mg_raw(dir, &["cat-file", "-p", &hex], None),
            "mg cat-file -p",
        );
        assert_eq!(
            pretty.stdout, payload,
            "pretty output is not byte identical"
        );
        assert!(pretty.stderr.is_empty());

        let kind = ok(
            mg_raw(dir, &["cat-file", "-t", &hex], None),
            "mg cat-file -t",
        );
        assert_eq!(kind.stdout, b"blob\n");

        let size = ok(
            mg_raw(dir, &["cat-file", "-s", &hex], None),
            "mg cat-file -s",
        );
        assert_eq!(size.stdout, format!("{}\n", payload.len()).into_bytes());

        // mg 写出的对象，真实 git 也必须读得出来（双向互操作）。
        if git_available() {
            let expected = git_raw(dir, &["hash-object", "--stdin"], Some(payload));
            assert_eq!(String::from_utf8_lossy(&expected.stdout).trim(), hex);
            let git_pretty = git_out(dir, &["cat-file", "-p", &hex]);
            assert!(git_pretty.status.success());
            assert_eq!(git_pretty.stdout, payload);
        }
    }

    // ------------------------------------------- B. 真 git 写、mg 读

    /// 对真实 git 仓库：`mg cat-file -p/-t/-s` 与 `git cat-file` 逐字节一致（blob/tree/commit/tag）。
    #[test]
    fn cat_file_matches_real_git_for_every_kind() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_git_repo(dir);

        let blob = oid_at(dir, "HEAD:a.txt");
        let tree = oid_at(dir, "HEAD^{tree}");
        let commit = oid_at(dir, "HEAD");
        let tag = oid_at(dir, "refs/tags/v1"); // annotated tag 对象
        assert_eq!(git(dir, &["cat-file", "-t", &tag]), "tag");

        for hex in [&blob, &tree, &commit, &tag] {
            for flag in ["-p", "-t", "-s"] {
                let ours = ok(mg_raw(dir, &["cat-file", flag, hex], None), "mg cat-file");
                let theirs = git_out(dir, &["cat-file", flag, hex]);
                assert!(theirs.status.success(), "git cat-file {flag} {hex} failed");
                assert_eq!(
                    ours.stdout, theirs.stdout,
                    "`mg cat-file {flag} {hex}` != `git cat-file {flag} {hex}`"
                );
            }
        }

        // rev 解析：HEAD / 短分支名 / 全名都走 `RefStore::resolve`，结果与 git 相同。
        let expected_head = git_out(dir, &["cat-file", "-p", "HEAD"]).stdout;
        for rev in ["HEAD", "main", "refs/heads/main"] {
            let ours = ok(mg_raw(dir, &["cat-file", "-p", rev], None), "mg cat-file");
            assert_eq!(ours.stdout, expected_head, "rev {rev}");
        }
        // annotated tag 也能按短名解析（T2 的 resolve 先 heads 后 tags）。
        let ours = ok(mg_raw(dir, &["cat-file", "-p", "v1"], None), "mg cat-file");
        assert_eq!(ours.stdout, git_out(dir, &["cat-file", "-p", "v1"]).stdout);
    }

    // ------------------------------------------- C. tree 的 ls-tree 格式

    /// 验收 6 的核心：`mg cat-file -p <tree>` == `git ls-tree <tree>`，逐字节（含 tab 与换行）。
    #[test]
    fn tree_pretty_output_is_byte_identical_to_ls_tree() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_git_repo(dir);

        let mut checked = 0;
        for rev in ["HEAD^{tree}", "HEAD:foo"] {
            let tree = oid_at(dir, rev);
            let ours = ok(mg_raw(dir, &["cat-file", "-p", &tree], None), "mg cat-file");
            let theirs = git_out(dir, &["ls-tree", &tree]);
            assert!(theirs.status.success());
            assert_eq!(
                ours.stdout, theirs.stdout,
                "tree {tree} ({rev}) differs from git ls-tree"
            );
            // 也跟 `git cat-file -p <tree>` 比一遍（同一个 pretty 打印器）。
            assert_eq!(ours.stdout, git_out(dir, &["cat-file", "-p", &tree]).stdout);
            // 内容检查：每一行都是 `<mode> <type> <oid>\t<name>`，且以换行结尾。
            let text = String::from_utf8(ours.stdout.clone()).unwrap();
            assert!(text.contains('\t'));
            assert_eq!(
                text.lines().count(),
                text.matches('\t').count(),
                "each entry needs exactly one tab: {text:?}"
            );
            assert!(text.ends_with('\n'), "entries must end with a newline");
            assert!(!text.is_empty());
            checked += 1;
        }
        assert_eq!(checked, 2);

        // 子树那一行必须是 git 的显示写法 `040000 tree <oid>\tfoo`。
        let tree = oid_at(dir, "HEAD^{tree}");
        let text = String::from_utf8(
            ok(mg_raw(dir, &["cat-file", "-p", &tree], None), "mg cat-file")
                .stdout
                .clone(),
        )
        .unwrap();
        let subtree_line = text
            .lines()
            .find(|line| line.ends_with("\tfoo"))
            .expect("subtree entry missing");
        assert!(
            subtree_line.starts_with("040000 tree "),
            "subtree mode must be printed the way git does: {subtree_line:?}"
        );
    }

    // ------------------------------------------- D. -e 的退出码

    /// `-e`：存在 → exit 0 且无输出；不存在 → exit 1 且报错走 stderr。
    #[test]
    fn exists_flag_reports_through_the_exit_code() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_git_repo(dir);
        let blob = oid_at(dir, "HEAD:a.txt");

        let found = mg_raw(dir, &["cat-file", "-e", &blob], None);
        assert!(found.status.success(), "existing object must exit 0");
        assert!(found.stdout.is_empty(), "-e must not print anything");
        assert!(found.stderr.is_empty(), "-e must not print anything");

        // 40 位 hex 但库里没有：resolve 会原样返回 oid，然后由存在性检查报错。
        let missing = Oid::hash_object("blob", b"definitely not in this repo").to_hex();
        let not_found = mg_raw(dir, &["cat-file", "-e", &missing], None);
        assert_eq!(not_found.status.code(), Some(1), "-e must exit 1");
        assert!(not_found.stdout.is_empty());
        assert!(
            !not_found.stderr.is_empty(),
            "-e must explain the failure on stderr"
        );

        // 未出生分支上的 HEAD：rev 解析失败也是非零退出（与 git 的 "Not a valid object name" 同类）。
        let empty = tempfile::tempdir().unwrap();
        Repo::init(empty.path(), DEFAULT_INITIAL_BRANCH).unwrap();
        let unborn = mg_raw(empty.path(), &["cat-file", "-t", "HEAD"], None);
        assert!(!unborn.status.success());
        assert!(String::from_utf8_lossy(&unborn.stderr).contains("HEAD"));

        // 不存在的 oid 用 `-p` 读：非零退出 + stderr 有信息 + **绝不能 panic**。
        let broken = mg_raw(dir, &["cat-file", "-p", &missing], None);
        assert_eq!(broken.status.code(), Some(1));
        assert!(broken.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&broken.stderr);
        assert!(stderr.contains("object not found"), "stderr was {stderr:?}");
        assert!(!stderr.contains("panicked"), "must not panic: {stderr}");
        // 真实 git 对这个 oid 也是非零退出（退出码 128，mg 走统一错误路径给 1）。
        assert!(!git_raw(dir, &["cat-file", "-p", &missing], None)
            .status
            .success());
    }

    // ------------------------------------------- E. 用法错误与 git 对齐

    /// 不给选择器时，真实 git 报用法错误而不是默认 pretty —— mg 同样报错（退出码不同，git 是 129）。
    #[test]
    fn missing_selector_is_a_usage_error_like_git() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_git_repo(dir);
        let blob = oid_at(dir, "HEAD:a.txt");

        let theirs = git_out(dir, &["cat-file", &blob]);
        assert!(!theirs.status.success(), "git should reject a bare oid");

        let ours = mg_raw(dir, &["cat-file", &blob], None);
        assert!(!ours.status.success(), "mg should reject a bare oid too");
        assert!(String::from_utf8_lossy(&ours.stderr).contains("usage"));
        assert!(ours.stdout.is_empty());
    }
}
