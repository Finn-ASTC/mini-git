//! `mg diff` —— **T6（omp）**。引擎在 `diff::`。
//!
//! 四种组合：工作区 vs index（默认）、index vs HEAD（`--staged`）、
//! commit vs 工作区（给 rev）、commit vs commit。
//! 验收：与 `git diff` 逐字节一致（含 `index` 行、`\ No newline`、二进制提示）。
//!
//! # 组合与候选路径
//!
//! | 命令行 | 旧侧 | 新侧 | 候选路径（=「哪一侧会出现这个文件」） |
//! |---|---|---|---|
//! | `mg diff` | index | 工作区 | index |
//! | `mg diff --staged` | HEAD | index | HEAD 树 ∪ index |
//! | `mg diff <rev>` | rev 的树 | 工作区 | rev 树 ∪ index |
//! | `mg diff --staged <rev>` | rev 的树 | index | rev 树 ∪ index |
//!
//! 「工作区」永远不是候选路径的来源，这正好复刻 git 的语义：未跟踪文件
//! **不会**出现在 `git diff` / `git diff <rev>` 里（唯一例外是 `--no-index`，
//! 不在 v1 范围）。
//!
//! # 头部格式（与 git 逐字节对齐）
//!
//! ```text
//! diff --git a/x b/x
//! [new file mode 100644 | deleted file mode 100644 | old mode .. + new mode ..]
//! index <old7>..<new7>[ <mode>]
//! --- a/x | /dev/null
//! +++ b/x | /dev/null
//! <unified diff | Binary files a/x and b/x differ>
//! ```
//!
//! * `index` 行只在两侧 oid 不同时出现；`<mode>` 后缀只在两侧都存在且 mode 相同时出现。
//! * 只有 mode 变化（内容相同）时，只打印 `old mode` / `new mode` 两行。
//! * 路径含 `"`、`\`、控制字节或 ≥ 0x7f 的字节时按 git 的 C 风格引号规则转义
//!   （`core.quotePath=true` 的默认行为），否则原样输出。
//! * 内容是原始字节：非 UTF-8 也原样输出（git 不做编码转换）。
//!
//! # 已知限制（都是非目标或上游能力边界）
//!
//! * **不做 rename/copy 检测**（任务书 §5 的非目标）：重命名会显示成「删除 + 新建」，
//!   与 `git diff --no-renames` 一致，与默认的 `git diff`（`diff.renames=true`）不同。
//! * `rev` 的解析交给 `refs::RefStore::resolve`：支持 `HEAD`、完整 40 位 oid、
//!   `refs/...`、分支名、tag 名（附注 tag 会被剥一层）。`HEAD~1` / `HEAD^` 这类
//!   修订表达式**不在** v1 范围（`reference not found`）。
//! * index 里 stage 1/2/3 的冲突条目会被跳过（冲突 diff 不在 v1 范围）。
//! * `paths` 是**前缀匹配**（任务书 §3.3 的放宽）；`:(exclude)` 等 pathspec magic 不支持。
//! * 不读 `core.quotePath` 等配置：始终按 git 的默认值（`core.quotePath=true`）转义路径。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry};
use crate::object::{FileMode, Object};
use crate::odb::Odb;
use crate::oid::Oid;
use crate::refs::RefStore;
use crate::repo::Repo;
use crate::worktree::Worktree;

use crate::diff::unified::unified_bytes;

/// git 判定二进制时只看前 8000 字节（`FIRST_FEW_BYTES`）。
const FIRST_FEW_BYTES: usize = 8000;

pub fn run(staged: bool, context: usize, rev: Option<&str>, paths: &[PathBuf]) -> Result<()> {
    let repo = super::open_repo()?;
    let bytes = render(&repo, staged, context, rev, paths)?;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    lock.write_all(&bytes)?;
    lock.flush()?;
    Ok(())
}

/// 一侧的内容来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// 某个提交的树（HEAD 或 `rev`）。
    Commit,
    Index,
    Worktree,
}

/// 参与比较的一个条目（`bytes` 只有工作区侧会预先读好）。
struct Entry {
    oid: Oid,
    mode: FileMode,
    bytes: Option<Vec<u8>>,
}

/// 生成整段 diff 输出（`run` 只负责把它写到 stdout）。
pub(crate) fn render(
    repo: &Repo,
    staged: bool,
    context: usize,
    rev: Option<&str>,
    paths: &[PathBuf],
) -> Result<Vec<u8>> {
    let index = Index::read(repo)?;
    let odb = Odb::new(repo);
    let worktree = Worktree::new(repo);

    let (old_source, commit_oid) = match rev {
        Some(name) => (Source::Commit, Some(resolve_commit(repo, name)?)),
        None if staged => (Source::Commit, Some(head_commit(repo)?).flatten()),
        None => (Source::Index, None),
    };
    let new_source = if staged {
        Source::Index
    } else {
        Source::Worktree
    };

    let commit_tree = match commit_oid {
        Some(oid) => flatten_commit(&odb, oid)?,
        None => BTreeMap::new(),
    };
    let index_map: BTreeMap<&[u8], &IndexEntry> = index
        .entries
        .iter()
        .filter(|entry| entry.stage == 0)
        .map(|entry| (entry.path.as_slice(), entry))
        .collect();

    // 候选路径 = 旧侧来源 ∪ 新侧来源。工作区侧不贡献路径（未跟踪文件不出现）。
    let mut candidates: BTreeSet<Vec<u8>> = BTreeSet::new();
    if old_source == Source::Commit {
        candidates.extend(commit_tree.keys().cloned());
    }
    if old_source == Source::Index || new_source == Source::Index || new_source == Source::Worktree
    {
        candidates.extend(index_map.keys().map(|path| path.to_vec()));
    }

    let filters = path_filters(paths);
    let mut out = Vec::new();
    for path in candidates {
        if !selected(&path, &filters) {
            continue;
        }
        let old = entry_of(old_source, &path, &commit_tree, &index_map, &worktree)?;
        let new = entry_of(new_source, &path, &commit_tree, &index_map, &worktree)?;

        // 内容（oid）与 mode 都没变 → 这个路径不出现在 diff 里。
        match (&old, &new) {
            (None, None) => continue,
            (Some(o), Some(n)) if o.oid == n.oid && o.mode == n.mode => continue,
            _ => {}
        }

        let old_bytes = match &old {
            Some(entry) => content(entry, &odb)?,
            None => Vec::new(),
        };
        let new_bytes = match &new {
            Some(entry) => content(entry, &odb)?,
            None => Vec::new(),
        };
        render_pair(
            &mut out,
            &path,
            old.as_ref(),
            new.as_ref(),
            &old_bytes,
            &new_bytes,
            context,
        )?;
    }

    Ok(out)
}

fn entry_of(
    source: Source,
    path: &[u8],
    commit_tree: &BTreeMap<Vec<u8>, (Oid, FileMode)>,
    index_map: &BTreeMap<&[u8], &IndexEntry>,
    worktree: &Worktree,
) -> Result<Option<Entry>> {
    match source {
        Source::Commit => Ok(commit_tree.get(path).map(|(oid, mode)| Entry {
            oid: *oid,
            mode: *mode,
            bytes: None,
        })),
        Source::Index => Ok(index_map.get(path).map(|entry| Entry {
            oid: entry.oid,
            mode: entry.mode,
            bytes: None,
        })),
        Source::Worktree => match worktree.read_worktree_entry(path) {
            Ok(content) => Ok(Some(Entry {
                oid: Oid::hash_object("blob", &content.bytes),
                mode: content.mode,
                bytes: Some(content.bytes),
            })),
            Err(Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        },
    }
}

fn content(entry: &Entry, odb: &Odb) -> Result<Vec<u8>> {
    match &entry.bytes {
        Some(bytes) => Ok(bytes.clone()),
        None => Ok(odb.read(entry.oid)?.1),
    }
}

fn render_pair(
    out: &mut Vec<u8>,
    path: &[u8],
    old: Option<&Entry>,
    new: Option<&Entry>,
    old_bytes: &[u8],
    new_bytes: &[u8],
    context: usize,
) -> Result<()> {
    let a_path = quote_path(b"a/", path);
    let b_path = quote_path(b"b/", path);
    out.extend_from_slice(b"diff --git ");
    out.extend_from_slice(&a_path);
    out.push(b' ');
    out.extend_from_slice(&b_path);
    out.push(b'\n');

    match (old, new) {
        (None, Some(entry)) => {
            out.extend_from_slice(format!("new file mode {}\n", entry.mode.as_str()).as_bytes());
        }
        (Some(entry), None) => {
            out.extend_from_slice(
                format!("deleted file mode {}\n", entry.mode.as_str()).as_bytes(),
            );
        }
        (Some(o), Some(n)) if o.mode != n.mode => {
            out.extend_from_slice(format!("old mode {}\n", o.mode.as_str()).as_bytes());
            out.extend_from_slice(format!("new mode {}\n", n.mode.as_str()).as_bytes());
        }
        _ => {}
    }

    let old_oid = old.map(|entry| entry.oid);
    let new_oid = new.map(|entry| entry.oid);
    if old_oid != new_oid {
        out.extend_from_slice(b"index ");
        push_short_oid(out, old_oid);
        out.extend_from_slice(b"..");
        push_short_oid(out, new_oid);
        if let (Some(o), Some(n)) = (old, new) {
            if o.mode == n.mode {
                out.push(b' ');
                out.extend_from_slice(o.mode.as_str().as_bytes());
            }
        }
        out.push(b'\n');
    }

    if old_bytes != new_bytes {
        let a_label = if old.is_some() {
            a_path.clone()
        } else {
            b"/dev/null".to_vec()
        };
        let b_label = if new.is_some() {
            b_path.clone()
        } else {
            b"/dev/null".to_vec()
        };
        if is_binary(old_bytes) || is_binary(new_bytes) {
            out.extend_from_slice(b"Binary files ");
            out.extend_from_slice(&a_label);
            out.extend_from_slice(b" and ");
            out.extend_from_slice(&b_label);
            out.extend_from_slice(b" differ\n");
        } else {
            let a_label = String::from_utf8(a_label)
                .map_err(|_| Error::Other("path label is not valid UTF-8".to_string()))?;
            let b_label = String::from_utf8(b_label)
                .map_err(|_| Error::Other("path label is not valid UTF-8".to_string()))?;
            out.extend_from_slice(&unified_bytes(
                old_bytes, new_bytes, &a_label, &b_label, context,
            )?);
        }
    }

    Ok(())
}

fn push_short_oid(out: &mut Vec<u8>, oid: Option<Oid>) {
    match oid {
        Some(oid) => {
            let hex = oid.to_hex();
            out.extend_from_slice(&hex.as_bytes()[..7]);
        }
        // git 用 7 个 0 表示「这一侧不存在」。
        None => out.extend_from_slice(b"0000000"),
    }
}

/// `buffer_is_binary`：前 8000 字节里出现 NUL 就算二进制。
fn is_binary(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(FIRST_FEW_BYTES)];
    window.contains(&0)
}

/// git 的路径引号规则（`quote_c_style` + `core.quotePath=true`）：
/// 只有 `"`、`\`、< 0x20、0x7f、≥ 0x80 需要转义；其余原样。
/// 需要转义时整体加双引号（含 `a/` 前缀，和 git 一样）。
fn quote_path(prefix: &[u8], path: &[u8]) -> Vec<u8> {
    let needs_quote = path.iter().any(|byte| {
        *byte < 0x20 || *byte == 0x7f || *byte >= 0x80 || *byte == b'"' || *byte == b'\\'
    });
    let mut out = Vec::with_capacity(prefix.len() + path.len() + 2);
    if needs_quote {
        out.push(b'"');
    }
    out.extend_from_slice(prefix);
    for &byte in path {
        if needs_quote {
            match byte {
                b'"' => out.extend_from_slice(b"\\\""),
                b'\\' => out.extend_from_slice(b"\\\\"),
                0x07 => out.extend_from_slice(b"\\a"),
                0x08 => out.extend_from_slice(b"\\b"),
                0x0c => out.extend_from_slice(b"\\f"),
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\r' => out.extend_from_slice(b"\\r"),
                b'\t' => out.extend_from_slice(b"\\t"),
                0x0b => out.extend_from_slice(b"\\v"),
                control if control < 0x20 || control == 0x7f || control >= 0x80 => {
                    out.extend_from_slice(format!("\\{:03o}", control).as_bytes());
                }
                other => out.push(other),
            }
        } else {
            out.push(byte);
        }
    }
    if needs_quote {
        out.push(b'"');
    }
    out
}

/// `-- <paths>` 过滤：本版本做**前缀匹配**（见任务书 §3.3）。
fn path_filters(paths: &[PathBuf]) -> Vec<Vec<u8>> {
    paths
        .iter()
        .map(|path| {
            let mut bytes = path_bytes(path);
            while bytes.last() == Some(&b'/') {
                bytes.pop();
            }
            bytes
        })
        .filter(|bytes| !bytes.is_empty())
        .collect()
}

fn selected(path: &[u8], filters: &[Vec<u8>]) -> bool {
    filters.is_empty() || filters.iter().any(|filter| path.starts_with(filter))
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

/// 把 `rev` 解析成提交 oid（附注 tag 会被剥一层）。
fn resolve_commit(repo: &Repo, name: &str) -> Result<Oid> {
    let oid = RefStore::new(repo).resolve(name)?;
    peel_commit(&Odb::new(repo), oid)
}

/// `HEAD` 指向的提交；仓库还没有提交时为 `None`。
fn head_commit(repo: &Repo) -> Result<Option<Oid>> {
    match RefStore::new(repo).resolve("HEAD") {
        Ok(oid) => Ok(Some(oid)),
        Err(Error::RefNotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

/// 接受提交 oid，或指向提交的附注 tag。
fn peel_commit(odb: &Odb, oid: Oid) -> Result<Oid> {
    match odb.read_object(oid)? {
        Object::Commit(_) => Ok(oid),
        Object::Tag(tag) => {
            if tag.kind != crate::object::Kind::Commit {
                return Err(Error::Other(format!(
                    "cannot diff against tag {oid}: it points at a {}",
                    tag.kind
                )));
            }
            Ok(tag.object)
        }
        other => Err(Error::Other(format!(
            "cannot diff against {oid}: expected commit, got {}",
            other.kind()
        ))),
    }
}

/// 把提交的树展平成 `路径 → (oid, mode)`（与 `worktree::status` 的约定一致）。
fn flatten_commit(odb: &Odb, commit_oid: Oid) -> Result<BTreeMap<Vec<u8>, (Oid, FileMode)>> {
    let commit = odb.read_object(commit_oid)?.into_commit()?;
    let mut out = BTreeMap::new();
    flatten_tree(odb, commit.tree, &mut Vec::new(), &mut out)?;
    Ok(out)
}

fn flatten_tree(
    odb: &Odb,
    tree_oid: Oid,
    prefix: &mut Vec<u8>,
    out: &mut BTreeMap<Vec<u8>, (Oid, FileMode)>,
) -> Result<()> {
    let tree = odb.read_object(tree_oid)?.into_tree()?;
    for entry in tree.entries() {
        let saved = prefix.len();
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(&entry.name);
        if entry.mode.is_tree() {
            flatten_tree(odb, entry.oid, prefix, out)?;
        } else {
            out.insert(prefix.clone(), (entry.oid, entry.mode));
        }
        prefix.truncate(saved);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::{Command, Output};

    /// 隔离的临时仓库：所有 git 调用都关掉全局/系统配置并固定身份与时间戳。
    struct Scratch {
        dir: tempfile::TempDir,
    }

    impl Scratch {
        fn new() -> Scratch {
            Scratch {
                dir: tempfile::tempdir().expect("tempdir"),
            }
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn git(&self, args: &[&str]) -> Output {
            Command::new("git")
                .current_dir(self.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "Test Author")
                .env("GIT_AUTHOR_EMAIL", "author@example.com")
                .env("GIT_COMMITTER_NAME", "Test Author")
                .env("GIT_COMMITTER_EMAIL", "author@example.com")
                .env("GIT_AUTHOR_DATE", "1700000000 +0800")
                .env("GIT_COMMITTER_DATE", "1700000000 +0800")
                .env("LC_ALL", "C")
                .args([
                    "-c",
                    "init.defaultBranch=main",
                    "-c",
                    "core.autocrlf=false",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "gc.auto=0",
                ])
                .args(args)
                .output()
                .expect("failed to spawn git")
        }

        fn git_ok(&self, args: &[&str]) -> Vec<u8> {
            let out = self.git(args);
            assert!(
                out.status.success(),
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let path = self.path().join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, bytes).unwrap();
        }

        fn repo(&self) -> Repo {
            Repo::discover(self.path()).expect("discover repo")
        }

        /// `mg diff` 的整条渲染管线（`run` 只是把它写到 stdout）。
        fn diff(&self, staged: bool, context: usize, rev: Option<&str>, paths: &[&str]) -> Vec<u8> {
            let repo = self.repo();
            let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            render(&repo, staged, context, rev, &paths).expect("render failed")
        }
    }

    /// 差分断言：失败时把双方原文都打出来（含转义与十六进制），便于事后复核。
    fn assert_same(what: &str, mg: &[u8], git: &[u8]) {
        if mg == git {
            return;
        }
        panic!(
            "{what}: byte mismatch\n--- git ({} bytes) ---\n{:?}\n{:?}\n--- mg ({} bytes) ---\n{:?}\n{:?}",
            git.len(),
            String::from_utf8_lossy(git),
            git,
            mg.len(),
            String::from_utf8_lossy(mg),
            mg
        );
    }

    /// 造一个「已提交 + 各种工作区改动」的仓库，返回 scratch。
    fn repo_with_changes() -> Scratch {
        let scratch = Scratch::new();
        scratch.git_ok(&["init", "--quiet", "."]);
        scratch.write("mod.txt", b"one\ntwo\nthree\n");
        scratch.write("del.txt", b"gone\n");
        scratch.write("mode.txt", b"script\n");
        scratch.write("bin.dat", b"\x00\x01\x02binary\n");
        scratch.write("dir/nested.txt", b"nested\n");
        scratch.write("中文.txt", "你好\n".as_bytes());
        scratch.git_ok(&["add", "-A"]);
        scratch.git_ok(&["commit", "--quiet", "-m", "init"]);

        // 内容修改、删除、只看 mode、二进制修改、未跟踪文件。
        scratch.write("mod.txt", b"one\nTWO\nthree\nfour\n");
        fs::remove_file(scratch.path().join("del.txt")).unwrap();
        scratch.write("bin.dat", b"\x00\x01\x03binary\n");
        scratch.write("untracked.txt", b"invisible\n");
        let mode = scratch.path().join("mode.txt");
        let mut perms = fs::metadata(&mode).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
        }
        fs::set_permissions(&mode, perms).unwrap();
        scratch
    }

    #[test]
    fn worktree_versus_index_matches_real_git() {
        let scratch = repo_with_changes();
        let mg = scratch.diff(false, 3, None, &[]);
        let want = scratch.git_ok(&["diff"]);
        assert_same("mg diff vs git diff", &mg, &want);
        // 未跟踪文件必须不可见（git 的语义）。
        assert!(!String::from_utf8_lossy(&mg).contains("untracked.txt"));
    }

    #[test]
    fn staged_versus_head_matches_real_git() {
        let scratch = repo_with_changes();
        scratch.write("new.txt", b"brand\nnew\n");
        scratch.write("new-empty.txt", b"");
        scratch.git_ok(&["add", "-A"]);
        let mg = scratch.diff(true, 3, None, &[]);
        let want = scratch.git_ok(&["diff", "--staged"]);
        assert_same("mg diff --staged vs git diff --staged", &mg, &want);
        assert!(String::from_utf8_lossy(&mg).contains("new file mode 100644"));
        assert!(String::from_utf8_lossy(&mg).contains("deleted file mode 100644"));
    }

    #[test]
    fn rev_versus_worktree_matches_real_git() {
        let scratch = repo_with_changes();
        scratch.write("new.txt", b"brand\nnew\n");
        scratch.git_ok(&["add", "new.txt"]); // 已暂存的新文件也应出现在 `git diff HEAD` 里
        let mg = scratch.diff(false, 3, Some("HEAD"), &[]);
        let want = scratch.git_ok(&["diff", "HEAD"]);
        assert_same("mg diff HEAD vs git diff HEAD", &mg, &want);

        // 具名提交 / 分支名同样可解析。
        let mg2 = scratch.diff(false, 3, Some("main"), &[]);
        assert_same(
            "mg diff main vs git diff main",
            &mg2,
            &scratch.git_ok(&["diff", "main"]),
        );
    }

    #[test]
    fn context_variants_match_real_git() {
        let scratch = repo_with_changes();
        for context in [0usize, 1, 2, 3, 5] {
            let mg = scratch.diff(false, context, None, &[]);
            let want = scratch.git_ok(&["diff", &format!("--unified={context}")]);
            assert_same(&format!("mg diff -U{context}"), &mg, &want);
        }
    }

    #[test]
    fn path_filter_matches_real_git() {
        let scratch = repo_with_changes();
        let mg = scratch.diff(false, 3, None, &["mod.txt"]);
        let want = scratch.git_ok(&["diff", "--", "mod.txt"]);
        assert_same("mg diff mod.txt", &mg, &want);

        let mg = scratch.diff(false, 3, None, &["dir/"]);
        let want = scratch.git_ok(&["diff", "--", "dir/"]);
        assert_same("mg diff dir/", &mg, &want);
    }

    #[test]
    fn binary_and_non_utf8_content_match_real_git() {
        let scratch = repo_with_changes();
        // 非 UTF-8（Latin-1）文本：没有 NUL，所以不是二进制，必须原样输出字节。
        scratch.write("latin.txt", b"caf\xe9\n");
        scratch.git_ok(&["add", "latin.txt"]);
        scratch.git_ok(&["commit", "--quiet", "-m", "latin"]);
        scratch.write("latin.txt", b"caf\xe8\n");
        // 非 UTF-8 文件名（CJK）也要真的出现在 diff 里，才能验证路径引号。
        scratch.write("中文.txt", "你好\n地球\n".as_bytes());

        let mg = scratch.diff(false, 3, None, &[]);
        let want = scratch.git_ok(&["diff"]);
        assert_same("mg diff (binary + non-UTF-8)", &mg, &want);
        assert!(
            String::from_utf8_lossy(&mg).contains("Binary files a/bin.dat and b/bin.dat differ")
        );
        // 非 UTF-8 路径（含 ≥0x80 字节）必须按 git 的 C 风格引号转义。
        assert!(String::from_utf8_lossy(&mg).contains(
            r#"diff --git "a/\344\270\255\346\226\207.txt" "b/\344\270\255\346\226\207.txt""#
        ));
    }

    #[test]
    fn unborn_head_staged_matches_real_git() {
        let scratch = Scratch::new();
        scratch.git_ok(&["init", "--quiet", "."]);
        scratch.write("a.txt", b"hello\n");
        scratch.write("b/c.txt", b"world\n");
        scratch.git_ok(&["add", "-A"]);
        let mg = scratch.diff(true, 3, None, &[]);
        let want = scratch.git_ok(&["diff", "--staged"]);
        assert_same("mg diff --staged (unborn HEAD)", &mg, &want);
    }

    #[test]
    fn clean_repository_prints_nothing() {
        let scratch = Scratch::new();
        scratch.git_ok(&["init", "--quiet", "."]);
        scratch.write("a.txt", b"hello\n");
        scratch.git_ok(&["add", "-A"]);
        scratch.git_ok(&["commit", "--quiet", "-m", "init"]);
        assert!(scratch.diff(false, 3, None, &[]).is_empty());
        assert!(scratch.diff(true, 3, None, &[]).is_empty());
        assert!(scratch.diff(false, 3, Some("HEAD"), &[]).is_empty());
    }

    #[test]
    fn mode_only_change_matches_real_git() {
        let scratch = Scratch::new();
        scratch.git_ok(&["init", "--quiet", "."]);
        scratch.write("script.sh", b"#!/bin/sh\necho hi\n");
        scratch.git_ok(&["add", "-A"]);
        scratch.git_ok(&["commit", "--quiet", "-m", "init"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = scratch.path().join("script.sh");
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }
        let mg = scratch.diff(false, 3, None, &[]);
        let want = scratch.git_ok(&["diff"]);
        assert_same("mg diff (mode-only change)", &mg, &want);
        assert!(String::from_utf8_lossy(&mg).contains("old mode 100644\nnew mode 100755\n"));
    }

    #[test]
    fn path_quoting_matches_git_rules() {
        // 与 `git diff` 的实测输出对齐（见实现注释里的探针结果）。
        assert_eq!(quote_path(b"a/", b"plain.txt"), b"a/plain.txt".to_vec());
        assert_eq!(
            quote_path(b"a/", b"with space.txt"),
            b"a/with space.txt".to_vec()
        );
        assert_eq!(
            quote_path(b"a/", b"tab\there"),
            b"\"a/tab\\there\"".to_vec()
        );
        assert_eq!(quote_path(b"a/", b"quo\"te"), b"\"a/quo\\\"te\"".to_vec());
        assert_eq!(
            quote_path(b"a/", b"back\\slash"),
            b"\"a/back\\\\slash\"".to_vec()
        );
        assert_eq!(
            quote_path(b"a/", "中文".as_bytes()),
            b"\"a/\\344\\270\\255\\346\\226\\207\"".to_vec()
        );
        assert_eq!(quote_path(b"a/", &[0x7f]), b"\"a/\\177\"".to_vec());
    }

    #[test]
    fn binary_detection_uses_the_first_8000_bytes() {
        let mut text = vec![b'a'; 9000];
        text[8999] = 0;
        assert!(
            !is_binary(&text),
            "NUL 在第 8000 字节之后不算二进制（与 git 一致）"
        );
        text[7999] = 0;
        assert!(is_binary(&text));
        assert!(!is_binary(b"plain text\n"));
    }
}
