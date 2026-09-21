//! V1 —— 独立验证 T1（object 层）。
//!
//! 本 target 的期望值全部在**运行时**由真实 git 产出（`git write-tree` /
//! `git cat-file` / `git rev-parse`），不硬编码任何 golden 常量；
//! `minigit` 算出的字节与 oid 必须与 git 完全一致。

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use minigit::object::{hash, Commit, FileMode, Kind, Tag, Tree, TreeEntry};
use minigit::Oid;

/// 每个用例一个独立临时仓库；用完（含 panic 展开时）自动删除。
struct Scratch {
    dir: PathBuf,
}

static SEQ: AtomicUsize = AtomicUsize::new(0);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "minigit-verify-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let scratch = Scratch { dir };
        scratch.git(&["init", "-q", "-b", "main"]);
        scratch
    }

    fn git_raw(&self, args: &[&str]) -> Output {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_AUTHOR_DATE", "1700000000 +0800")
            .env("GIT_COMMITTER_DATE", "1700000000 +0800")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("failed to spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn git(&self, args: &[&str]) -> String {
        String::from_utf8(self.git_raw(args).stdout).expect("git stdout is UTF-8")
    }

    fn write(&self, rel: &str, contents: &[u8]) {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(&path, contents).expect("write scratch file");
    }

    fn commit(&self, message: &str) {
        self.git(&[
            "-c",
            "user.name=A U Thor",
            "-c",
            "user.email=a@example.com",
            "commit",
            "-q",
            "-m",
            message,
        ]);
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// `git rev-parse <rev>:<path>` —— 文件给 blob oid，目录给 tree oid。
    fn oid_at(&self, rev: &str, path: &str) -> Oid {
        let hex = self.git(&["rev-parse", &format!("{rev}:{path}")]);
        Oid::from_hex(hex.trim()).expect("git printed a valid oid")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn entry(mode: FileMode, name: &str, oid: Oid) -> TreeEntry {
    TreeEntry {
        mode,
        name: name.as_bytes().to_vec(),
        oid,
    }
}

/// (D.1 + D.2) 排序陷阱：同层同时有 `foo.txt`(文件) / `foo/`(子树) /
/// `foo-bar` / 深层 `foo/a/b`。oid 与字节都必须和真实 git 相同，
/// 并且 git 的原始字节 decode→encode 逐字节还原。
#[test]
fn tree_sort_trap_and_raw_bytes_match_real_git() {
    let scratch = Scratch::new("tree");
    scratch.write("foo.txt", b"c\n");
    scratch.write("foo-bar", b"dash\n");
    scratch.write("foo/a/b", b"deep\n");
    scratch.write("a.txt", b"a\n");
    scratch.git(&["add", "-A"]);

    let top_hex = scratch.git(&["write-tree"]).trim().to_string();
    let raw = scratch.git_raw(&["cat-file", "tree", &top_hex]).stdout;

    // 自底向上用 minigit 编出同一棵树；叶子 oid 直接取自真实 git。
    let leaf = Tree::new(vec![entry(
        FileMode::Regular,
        "b",
        scratch.oid_at(&top_hex, "foo/a/b"),
    )]);
    assert_eq!(
        Oid::hash_object("tree", &leaf.encode_payload()).to_hex(),
        scratch.oid_at(&top_hex, "foo/a").to_hex(),
        "subtree foo/a must hash exactly like git"
    );

    let foo = Tree::new(vec![entry(
        FileMode::Tree,
        "a",
        scratch.oid_at(&top_hex, "foo/a"),
    )]);
    assert_eq!(
        Oid::hash_object("tree", &foo.encode_payload()).to_hex(),
        scratch.oid_at(&top_hex, "foo").to_hex(),
        "subtree foo must hash exactly like git"
    );

    // 故意乱序：`foo` 子树放最前，`foo-bar` 和 `foo.txt` 紧随其后。
    let top = Tree::new(vec![
        entry(FileMode::Tree, "foo", scratch.oid_at(&top_hex, "foo")),
        entry(
            FileMode::Regular,
            "foo-bar",
            scratch.oid_at(&top_hex, "foo-bar"),
        ),
        entry(
            FileMode::Regular,
            "foo.txt",
            scratch.oid_at(&top_hex, "foo.txt"),
        ),
        entry(
            FileMode::Regular,
            "a.txt",
            scratch.oid_at(&top_hex, "a.txt"),
        ),
    ]);
    assert_eq!(
        Oid::hash_object("tree", &top.encode_payload()).to_hex(),
        top_hex,
        "Tree::encode_payload oid must equal git write-tree"
    );
    assert_eq!(
        top.encode_payload(),
        raw,
        "tree payload must equal `git cat-file tree` byte for byte"
    );

    // (D.2) 原始字节往返。
    let decoded = Tree::decode_payload(&raw).unwrap();
    assert_eq!(decoded.encode_payload(), raw);

    // `Tree::new` 不排序；decode 出来的是 git 的规范序，等于 sort_entries 的结果。
    let mut sorted_entries = top.entries().to_vec();
    Tree::sort_entries(&mut sorted_entries).unwrap();
    assert_eq!(decoded.entries(), sorted_entries.as_slice());

    // 顺序：`-`(0x2D) < `.`(0x2E) < 子树的 `/`(0x2F)。
    let names: Vec<&[u8]> = decoded
        .entries()
        .iter()
        .map(|e| e.name.as_slice())
        .collect();
    assert_eq!(
        names,
        vec![
            b"a.txt".as_slice(),
            b"foo-bar".as_slice(),
            b"foo.txt".as_slice(),
            b"foo".as_slice(),
        ],
        "foo.txt must sort before the foo subtree"
    );
    let from_git: Vec<String> = scratch
        .git(&["ls-tree", "--name-only", &top_hex])
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let decoded_names: Vec<String> = decoded
        .entries()
        .iter()
        .map(|e| String::from_utf8_lossy(&e.name).into_owned())
        .collect();
    assert_eq!(
        decoded_names, from_git,
        "decode order must match git ls-tree"
    );

    // sort_entries 必须把乱序切片排成 git 的规范序。
    let mut shuffled = top.entries().to_vec();
    shuffled.reverse();
    Tree::sort_entries(&mut shuffled).unwrap();
    assert_eq!(shuffled, sorted_entries);
}

/// (D.3) merge 提交：两个 parent 的原始字节往返，且 parents 顺序与 git 一致。
#[test]
fn merge_commit_roundtrips_against_real_git() {
    let scratch = Scratch::new("merge");
    scratch.write("x.txt", b"x\n");
    scratch.git(&["add", "-A"]);
    scratch.commit("base");

    scratch.git(&["checkout", "-q", "-b", "side"]);
    scratch.write("y.txt", b"y\n");
    scratch.git(&["add", "-A"]);
    scratch.commit("side");

    scratch.git(&["checkout", "-q", "main"]);
    scratch.write("z.txt", b"z\n");
    scratch.git(&["add", "-A"]);
    scratch.commit("main");

    scratch.git(&[
        "-c",
        "user.name=A U Thor",
        "-c",
        "user.email=a@example.com",
        "merge",
        "-q",
        "--no-ff",
        "side",
        "-m",
        "merge side",
    ]);

    let head = scratch.head();
    let raw = scratch.git_raw(&["cat-file", "commit", "HEAD"]).stdout;
    let commit = Commit::decode_payload(&raw).unwrap();

    assert!(commit.is_merge());
    assert_eq!(commit.parents.len(), 2, "a --no-ff merge has two parents");
    assert_eq!(
        commit.encode_payload(),
        raw,
        "commit roundtrip must be byte for byte"
    );
    assert_eq!(hash(Kind::Commit, &commit.encode_payload()).to_hex(), head);
    assert_eq!(
        commit.tree.to_hex(),
        scratch.git(&["rev-parse", "HEAD^{tree}"]).trim()
    );
    assert_eq!(commit.message, b"merge side\n");
    assert_eq!(commit.summary(), "merge side");
    assert_eq!(commit.author, commit.committer);
    assert_eq!(commit.author.name, b"A U Thor");
    assert_eq!(commit.author.email, b"a@example.com");
    assert_eq!(
        (commit.author.when, commit.author.tz.as_str()),
        (1_700_000_000, "+0800")
    );
    assert!(commit.extra_headers.is_empty());

    let from_git = scratch.git(&["rev-list", "--parents", "-n", "1", "HEAD"]);
    let mut tokens = from_git.split_whitespace();
    assert_eq!(tokens.next().unwrap(), head);
    let git_parents: Vec<String> = tokens.map(str::to_string).collect();
    let our_parents: Vec<String> = commit.parents.iter().map(Oid::to_hex).collect();
    assert_eq!(
        our_parents, git_parents,
        "first parent must come first, same as git rev-list --parents"
    );
}

/// (D.4) annotated tag 的原始字节往返。
#[test]
fn annotated_tag_roundtrips_against_real_git() {
    let scratch = Scratch::new("tag");
    scratch.write("x.txt", b"x\n");
    scratch.git(&["add", "-A"]);
    scratch.commit("base");
    scratch.git(&[
        "-c",
        "user.name=A U Thor",
        "-c",
        "user.email=a@example.com",
        "tag",
        "-a",
        "v1",
        "-m",
        "tag message",
    ]);

    let head = scratch.head();
    let tag_hex = scratch.git(&["rev-parse", "v1"]).trim().to_string();
    let raw = scratch.git_raw(&["cat-file", "tag", "v1"]).stdout;
    let tag = Tag::decode_payload(&raw).unwrap();

    assert_eq!(
        tag.encode_payload(),
        raw,
        "tag roundtrip must be byte for byte"
    );
    assert_eq!(hash(Kind::Tag, &tag.encode_payload()).to_hex(), tag_hex);
    assert_eq!(tag.object.to_hex(), head);
    assert_eq!(tag.kind, Kind::Commit);
    assert_eq!(tag.name, b"v1");
    assert_eq!(tag.message, b"tag message\n");
    assert!(tag.extra_headers.is_empty());
    let tagger = tag.tagger.expect("an annotated tag carries a tagger");
    assert_eq!(tagger.name, b"A U Thor");
    assert_eq!(tagger.email, b"a@example.com");
    assert_eq!((tagger.when, tagger.tz.as_str()), (1_700_000_000, "+0800"));
}
