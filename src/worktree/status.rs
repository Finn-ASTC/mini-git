//! 三方状态比对与 `status --porcelain` 渲染。**T4（codex）实现范围**。
//!
//! 为什么单独一个文件：`mod.rs` 是 CONTROLLER-OWNED（冻结公共签名），
//! 实现体必须放在模块自己的文件里，这样 author agent 永远不需要改冻结文件。
//! 本文件的 `pub` 签名来自 `mod.rs` 的 `struct`/`impl` 声明，**不允许改签名**。
//!
//! 语义（与真实 git 对拍得到）：
//! * `X` = HEAD（**已展平**的 tree）vs index stage 0：`A` / `M` / `D` / `T`，
//!   比较 oid 与 mode（`100644` ↔ `100755` 算 `M`，**类型位**变化算 `T`）。
//! * `Y` = index stage 0 vs 工作区：内容直接按 `blob` 哈希比较（不信任 stat），
//!   工作区里是目录或不存在算 `D`；mode 变化也算 `M`，**类型变化算 `T`**
//!   （用真实 git 对拍确认：index 是普通文件、工作区换成 symlink → ` T`，
//!   即使 symlink 的内容恰好等于原 blob 也仍是 `T`；反之亦然）。
//! * stage 1/2/3 的路径只输出冲突码（`UU/AA/AU/UA/DU/UD`），不再输出 X/Y。
//! * 只在工作区、不在 index（任何 stage）、且不被 ignore 的路径输出 `??`；
//!   若某个目录下没有任何 index 条目，则整个目录折叠成一条 `?? dir/`。
//! * `lines` 按 path 字节序排序；`porcelain()` 每行 `XY SP path LF`，末尾带换行，
//!   但**分组**与 git 一致：先 X/Y/冲突行（按 path），再 `??` 行（按 path）——
//!   git 的 `wt-status.c` 就是先打印 changed 再打印 untracked。
//!
//! 已知限制（写在这里，不在本轮实现）：
//! * 不做 rename 检测：把 `a` 改名成 `b` 在 mg 里是 `D a` + `A b`，git 可能输出
//!   `R  a -> b`（测试场景不构造 rename；如需要请用 `status.renames=false` 取真值）。
//! * 「工作区里的路径变成目录」**不是** `T`：git 对这种情况报 ` D`（已对拍确认），
//!   只有普通文件 ↔ symlink 这类同一路径上的类型替换才是 `T`。
//! * 不支持 `core.excludesFile` / `info/exclude` / skip-worktree / assume-valid /
//!   submodule / sparse checkout；intent-to-add 条目按普通 stage 0 处理。
//! * 路径渲染复刻 git 默认的 `core.quotePath=true`（含非 ASCII / 控制字节的路径
//!   按 C 风格加引号）；非 UTF-8 字节同样按 8 进制转义输出，不做 lossy 替换。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry};
use crate::object::{FileMode, Tree};
use crate::oid::Oid;

use super::{ChangeKind, ConflictKind, StatusLine, StatusReport, Worktree, WorktreeContent};

impl StatusLine {
    /// 两字符 XY 码，例如 `" M"` / `"MM"` / `"??"` / `"UU"`。
    pub fn xy(&self) -> String {
        if let Some(conflict) = self.conflict {
            return conflict.porcelain_code().to_string();
        }
        if self.untracked {
            return "??".to_string();
        }
        let mut out = String::with_capacity(2);
        out.push(self.index.map_or(' ', ChangeKind::code));
        out.push(self.worktree.map_or(' ', ChangeKind::code));
        out
    }
}

impl StatusReport {
    /// `git status --porcelain` v1 格式（含末尾换行）。
    ///
    /// 行的**分组**与 git 一致：先输出 X/Y/冲突行，再输出 `??` 行；每组内部按 path
    /// 字节序（`lines` 本身始终是全局 path 序，见 `mod.rs` 的字段说明）。
    pub fn porcelain(&self) -> String {
        let mut out = String::new();
        for line in self.lines.iter().filter(|line| !line.untracked) {
            push_porcelain_line(&mut out, line);
        }
        for line in self.lines.iter().filter(|line| line.untracked) {
            push_porcelain_line(&mut out, line);
        }
        out
    }
}

fn push_porcelain_line(out: &mut String, line: &StatusLine) {
    out.push_str(&line.xy());
    out.push(' ');
    out.push_str(&quote_path(&line.path));
    out.push('\n');
}

/// 复刻 `git status --porcelain`（v1，默认 `core.quotePath=true`）的路径渲染：
/// 含空格、控制字节、`"`、`\` 或非 ASCII 字节的路径按 C 风格加引号并在引号内转义，
/// 其余原样输出（经验证：`!` `#` `'` `[` 等字符不触发加引号）。
fn quote_path(path: &[u8]) -> String {
    // git 在短格式里把「含空格」也视为需要加引号（路径跟在 `XY ` 之后）。
    let needs_quote = path
        .iter()
        .any(|b| *b <= b' ' || *b == 0x7f || *b >= 0x80 || *b == b'"' || *b == b'\\');
    if !needs_quote {
        return String::from_utf8_lossy(path).into_owned();
    }

    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for &b in path {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x07 => out.push_str("\\a"),
            0x08 => out.push_str("\\b"),
            0x0c => out.push_str("\\f"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x0b => out.push_str("\\v"),
            b if b < 0x20 || b == 0x7f || b >= 0x80 => out.push_str(&format!("\\{b:03o}")),
            b => out.push(b as char),
        }
    }
    out.push('"');
    out
}

impl<'a> Worktree<'a> {
    /// 三方状态比对。`head_tree` 为 `None` 表示仓库还没有提交。
    pub fn status(&self, head_tree: Option<&Tree>, index: &Index) -> Result<StatusReport> {
        let mut head: BTreeMap<&[u8], (Oid, FileMode)> = BTreeMap::new();
        if let Some(tree) = head_tree {
            for entry in tree.entries() {
                // 防御：`head_tree` 应当是已展平的，子树条目直接忽略。
                if entry.mode == FileMode::Tree {
                    continue;
                }
                head.insert(entry.name.as_slice(), (entry.oid, entry.mode));
            }
        }

        let mut stage0: BTreeMap<&[u8], &IndexEntry> = BTreeMap::new();
        // path -> stage bitmask（stage1=1, stage2=2, stage3=4），与 git 的 stagemask 一致。
        let mut conflicts: BTreeMap<&[u8], u8> = BTreeMap::new();
        for entry in &index.entries {
            if entry.is_stage0() {
                stage0.insert(entry.path.as_slice(), entry);
            } else if (1..=3).contains(&entry.stage) {
                *conflicts.entry(entry.path.as_slice()).or_insert(0) |= 1u8 << (entry.stage - 1);
            }
        }

        let mut lines: Vec<StatusLine> = Vec::new();

        let mut paths: Vec<&[u8]> = head.keys().copied().chain(stage0.keys().copied()).collect();
        paths.sort_unstable();
        paths.dedup();

        for path in paths {
            // 冲突路径只输出冲突码。
            if conflicts.contains_key(path) {
                continue;
            }
            let x = match (head.get(path), stage0.get(path)) {
                (None, Some(_)) => Some(ChangeKind::Added),
                (Some(_), None) => Some(ChangeKind::Deleted),
                (Some((oid, mode)), Some(entry)) => {
                    if is_type_change(*mode, entry.mode) {
                        // 类型变化（普通文件 ↔ symlink）优先于内容比较：git 的
                        // diff-index 只看类型位，内容是否相同不影响 `T`。
                        Some(ChangeKind::TypeChanged)
                    } else if *oid != entry.oid || *mode != entry.mode {
                        Some(ChangeKind::Modified)
                    } else {
                        None
                    }
                }
                (None, None) => None,
            };

            let y = match stage0.get(path) {
                None => None,
                Some(entry) => self.worktree_change(path, entry)?,
            };

            if x.is_none() && y.is_none() {
                continue;
            }
            let mut line = StatusLine::new(path.to_vec());
            line.index = x;
            line.worktree = y;
            lines.push(line);
        }

        // 未跟踪：工作区里有、index 里（任何 stage）没有、且不被 ignore。
        let scanned = self.scan()?;
        let mut index_paths: Vec<Vec<u8>> = index
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        index_paths.sort();
        index_paths.dedup();
        let index_set: HashSet<&[u8]> = index_paths.iter().map(|p| p.as_slice()).collect();

        let mut untracked: BTreeSet<Vec<u8>> = BTreeSet::new();
        for file in &scanned {
            if file.ignored || file.is_dir {
                continue;
            }
            if index_set.contains(file.path.as_slice()) {
                continue;
            }
            // 祖先路径本身就是 index 条目（文件）时，git 不把目录里的东西算成
            // untracked（例：`f.txt` 被换成了目录 `f.txt/`，只报 ` D f.txt`）。
            if ancestor_prefixes(&file.path)
                .iter()
                .any(|ancestor| index_set.contains(*ancestor))
            {
                continue;
            }
            untracked.insert(render_untracked(&file.path, &index_paths));
        }
        for path in untracked {
            lines.push(StatusLine {
                path,
                index: None,
                worktree: None,
                conflict: None,
                untracked: true,
            });
        }

        for (path, mask) in &conflicts {
            lines.push(StatusLine {
                path: path.to_vec(),
                index: None,
                worktree: None,
                conflict: Some(conflict_kind(*mask)),
                untracked: false,
            });
        }

        lines.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(StatusReport { lines })
    }

    /// 读取工作区内容用于计算 blob（普通文件读字节，symlink 读链接目标），
    /// 并连同规范化后的 mode 一起返回。
    pub fn read_worktree_entry(&self, path: &[u8]) -> Result<WorktreeContent> {
        let abs = self.repo().work_path(path)?;
        let meta = fs::symlink_metadata(&abs)?;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&abs)?;
            // symlink 的内容就是链接目标（不含结尾 NUL），git 的 120000 blob 正是如此。
            return Ok(WorktreeContent {
                bytes: symlink_target_bytes(&target),
                mode: FileMode::Symlink,
            });
        }
        if meta.is_dir() {
            return Err(Error::Other(format!(
                "worktree path is a directory: {}",
                String::from_utf8_lossy(path)
            )));
        }
        let bytes = fs::read(&abs)?;
        Ok(WorktreeContent {
            bytes,
            mode: file_mode_from_metadata(&meta),
        })
    }

    /// index 条目 vs 工作区：`M`（内容或 mode 不同）、`D`（不存在、是目录、或祖先不是目录）。
    fn worktree_change(&self, path: &[u8], entry: &IndexEntry) -> Result<Option<ChangeKind>> {
        // 祖先被换成了普通文件时（例如 `dir/file` → 文件 `dir`），git 报 ` D`。
        for ancestor in ancestor_prefixes(path) {
            let abs = self.repo().work_path(ancestor)?;
            if let Ok(meta) = fs::symlink_metadata(&abs) {
                if !meta.is_dir() {
                    return Ok(Some(ChangeKind::Deleted));
                }
            }
        }
        let abs = self.repo().work_path(path)?;
        match fs::symlink_metadata(&abs) {
            Err(_) => Ok(Some(ChangeKind::Deleted)),
            Ok(meta) if meta.is_dir() => Ok(Some(ChangeKind::Deleted)),
            Ok(_) => {
                let content = self.read_worktree_entry(path)?;
                // 类型变化优先于内容比较（与 `X` 列同样的规则）：git 只要 mode 的
                // 类型位不同就报 `T`，哪怕 symlink 的内容撞巧等于原 blob。
                if is_type_change(entry.mode, content.mode) {
                    return Ok(Some(ChangeKind::TypeChanged));
                }
                let changed = content.mode != entry.mode
                    || Oid::hash_object("blob", &content.bytes) != entry.oid;
                Ok(changed.then_some(ChangeKind::Modified))
            }
        }
    }
}

/// git 的「类型变化」判定：只看 mode 的**类型位**，不看权限位。
///
/// `100644` ↔ `100755` 是权限变化（`M`）；普通文件/可执行 ↔ `120000`（symlink）
/// 与 ↔ `40000`（目录）才是类型变化（`T`）。`Y` 列里「工作区是目录」走 `D`，
/// 因此这里的 `Tree` 分支只影响 `X` 列（tree 条目本不该出现在 stage 0）。
fn is_type_change(a: FileMode, b: FileMode) -> bool {
    mode_class(a) != mode_class(b)
}

fn mode_class(mode: FileMode) -> u8 {
    match mode {
        FileMode::Regular | FileMode::Executable => 0,
        FileMode::Symlink => 1,
        FileMode::Tree => 2,
    }
}

/// 冲突的 stagemask（stage1=1 / stage2=2 / stage3=4）→ porcelain 码。
/// 与 git `wt-status.c` 的表一致；只有 base 的 mask=1（DD）在 `ConflictKind`
/// 里没有对应变体，按 `UU` 处理（正常 merge 不会产生这种状态）。
fn conflict_kind(mask: u8) -> ConflictKind {
    match mask {
        2 => ConflictKind::AddedByUs,
        4 => ConflictKind::AddedByThem,
        3 => ConflictKind::DeletedByThem,
        5 => ConflictKind::DeletedByUs,
        6 => ConflictKind::BothAdded,
        _ => ConflictKind::BothModified,
    }
}

/// `path` 的所有祖先目录前缀（component 边界，不含 `path` 自身）。
fn ancestor_prefixes(path: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    while let Some(offset) = path[idx..].iter().position(|b| *b == b'/') {
        idx += offset;
        out.push(&path[..idx]);
        idx += 1;
    }
    out
}

/// 未跟踪路径的渲染：若某层祖先目录下没有任何 index 条目，则折叠成 `dir/`。
fn render_untracked(path: &[u8], index_paths: &[Vec<u8>]) -> Vec<u8> {
    let components: Vec<&[u8]> = path.split(|b| *b == b'/').collect();
    if components.len() <= 1 {
        return path.to_vec();
    }
    let mut prefix: Vec<u8> = Vec::new();
    for (i, component) in components[..components.len() - 1].iter().enumerate() {
        if i > 0 {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(component);
        if !has_index_entry_under(index_paths, &prefix) {
            prefix.push(b'/');
            return prefix;
        }
    }
    path.to_vec()
}

/// `index_paths` 升序时判断是否存在以 `dir + "/"` 开头的条目。
fn has_index_entry_under(index_paths: &[Vec<u8>], dir: &[u8]) -> bool {
    let mut prefix = Vec::with_capacity(dir.len() + 1);
    prefix.extend_from_slice(dir);
    prefix.push(b'/');
    let idx = index_paths.partition_point(|p| p.as_slice() < prefix.as_slice());
    index_paths.get(idx).is_some_and(|p| p.starts_with(&prefix))
}

#[cfg(unix)]
fn symlink_target_bytes(target: &std::path::Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    target.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn symlink_target_bytes(target: &std::path::Path) -> Vec<u8> {
    target.to_string_lossy().into_owned().into_bytes()
}

fn file_mode_from_metadata(meta: &fs::Metadata) -> FileMode {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 != 0 {
            FileMode::Executable
        } else {
            FileMode::Regular
        }
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        FileMode::Regular
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    use super::*;
    use crate::repo::Repo;

    /// 一个用真实 `git` 建的临时仓库；所有真值都从它现场取。
    struct Scratch {
        dir: tempfile::TempDir,
    }

    impl Scratch {
        fn new() -> Scratch {
            let dir = tempfile::tempdir().expect("tempdir");
            let scratch = Scratch { dir };
            scratch.git_ok(&["init", "-q", "-b", "main"]);
            scratch
        }

        fn path(&self) -> &Path {
            self.dir.path()
        }

        fn join(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }

        fn write(&self, rel: &str, contents: &str) {
            let path = self.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(path, contents).expect("write file");
        }

        fn remove(&self, rel: &str) {
            std::fs::remove_file(self.join(rel)).expect("remove file");
        }

        fn git(&self, args: &[&str]) -> Output {
            Command::new("git")
                .args(args)
                .current_dir(self.dir.path())
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", self.dir.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_MERGE_AUTOEDIT", "no")
                .env("GIT_AUTHOR_NAME", "scenario")
                .env("GIT_AUTHOR_EMAIL", "scenario@example.com")
                .env("GIT_COMMITTER_NAME", "scenario")
                .env("GIT_COMMITTER_EMAIL", "scenario@example.com")
                .env("GIT_AUTHOR_DATE", "1700000000 +0800")
                .env("GIT_COMMITTER_DATE", "1700000000 +0800")
                .output()
                .expect("run git")
        }

        fn git_ok(&self, args: &[&str]) -> Vec<u8> {
            let out = self.git(args);
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        }

        fn commit_all(&self, message: &str) {
            self.git_ok(&["add", "-A"]);
            self.git_ok(&["commit", "-q", "-m", message]);
        }

        /// 真值：真实 git 的 `status --porcelain` 原始 stdout（含末尾换行）。
        fn git_status(&self) -> String {
            String::from_utf8(self.git_ok(&["status", "--porcelain"])).expect("utf8 status")
        }
    }

    /// 用 `git ls-tree -r -z HEAD` 构造**展平**的 tree（name 是仓库相对全路径）。
    fn load_head_tree(scratch: &Scratch) -> Option<Tree> {
        let probe = scratch.git(&["rev-parse", "--verify", "--quiet", "HEAD"]);
        if !probe.status.success() {
            return None;
        }
        let raw = scratch.git_ok(&["ls-tree", "-r", "-z", "HEAD"]);
        let mut entries = Vec::new();
        for record in raw.split(|b| *b == 0) {
            if record.is_empty() {
                continue;
            }
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .expect("ls-tree tab");
            let header = std::str::from_utf8(&record[..tab]).expect("ls-tree header utf8");
            let name = record[tab + 1..].to_vec();
            let mut fields = header.split_whitespace();
            let mode_field = fields.next().expect("mode");
            let _kind = fields.next().expect("type");
            let oid = Oid::from_hex(fields.next().expect("oid")).expect("oid");
            // `160000`（gitlink）等非 blob/tree 模式不在 v1 范围内，直接跳过。
            if let Ok(mode) = FileMode::from_bytes(mode_field.as_bytes()) {
                entries.push(crate::object::TreeEntry { mode, name, oid });
            }
        }
        Some(Tree::new(entries))
    }

    /// 用 `git ls-files --stage -z` 构造 index。
    fn load_index(scratch: &Scratch) -> Index {
        let raw = scratch.git_ok(&["ls-files", "--stage", "-z"]);
        let mut entries = Vec::new();
        for record in raw.split(|b| *b == 0) {
            if record.is_empty() {
                continue;
            }
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .expect("ls-files tab");
            let header = std::str::from_utf8(&record[..tab]).expect("ls-files header utf8");
            let path = record[tab + 1..].to_vec();
            let mut fields = header.split_whitespace();
            let mode = FileMode::from_bytes(fields.next().expect("mode").as_bytes()).expect("mode");
            let oid = Oid::from_hex(fields.next().expect("oid")).expect("oid");
            let stage: u8 = fields.next().expect("stage").parse().expect("stage number");
            let mut entry = IndexEntry::new(path, oid, mode);
            entry.stage = stage;
            entries.push(entry);
        }
        let mut index = Index {
            entries,
            ..Index::default()
        };
        index.sort();
        index
    }

    /// 调用被测的 `Worktree::status` 并渲染 porcelain。
    fn mg_status(scratch: &Scratch) -> String {
        let repo = Repo::discover(scratch.path()).expect("discover repo");
        let worktree = Worktree::new(&repo);
        let head = load_head_tree(scratch);
        let index = load_index(scratch);
        worktree
            .status(head.as_ref(), &index)
            .expect("status")
            .porcelain()
    }

    /// 硬标准：mg 的 porcelain 与真实 git 逐字节相同。
    fn assert_matches_git(scratch: &Scratch, scenario: &str) {
        let want = scratch.git_status();
        let got = mg_status(scratch);
        assert_eq!(got, want, "porcelain mismatch in scenario `{scenario}`");
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).expect("chmod");
    }

    #[test]
    fn empty_repo_matches_git() {
        let scratch = Scratch::new();
        assert!(scratch.git_status().is_empty());
        assert_eq!(mg_status(&scratch), "");
    }

    #[test]
    fn no_commit_repo_matches_git() {
        let scratch = Scratch::new();
        scratch.write("staged.txt", "staged\n");
        scratch.git_ok(&["add", "staged.txt"]);
        scratch.write("untracked.txt", "u\n");
        scratch.write("dir/x.txt", "x\n");
        scratch.write("dir/sub/y.txt", "y\n");
        assert_matches_git(&scratch, "no-commit");
    }

    #[test]
    fn rich_worktree_states_match_git() {
        let scratch = Scratch::new();
        scratch.write("modified.txt", "one\n");
        scratch.write("deleted.txt", "gone\n");
        scratch.write("staged.txt", "base\n");
        scratch.write("exec.sh", "#!/bin/sh\n");
        scratch.write("keep/old.txt", "old\n");
        scratch.write("partial/tracked.txt", "tracked\n");
        scratch.commit_all("init");

        scratch.write("modified.txt", "two\n");
        scratch.remove("deleted.txt");
        scratch.write("staged.txt", "staged change\n");
        scratch.git_ok(&["add", "staged.txt"]);
        scratch.write("keep/old.txt", "staged too\n");
        scratch.git_ok(&["add", "keep/old.txt"]);
        scratch.write("added-and-modified.txt", "a\n");
        scratch.git_ok(&["add", "added-and-modified.txt"]);
        scratch.write("added-and-modified.txt", "a changed\n");
        scratch.write("untracked.txt", "u\n");
        scratch.write("udir/a.txt", "a\n");
        scratch.write("udir/sub/b.txt", "b\n");
        scratch.write("partial/new.txt", "new again\n");

        assert_matches_git(&scratch, "rich-worktree-states");
    }

    #[cfg(unix)]
    #[test]
    fn exec_bit_change_matches_git() {
        let scratch = Scratch::new();
        scratch.write("run.sh", "#!/bin/sh\n");
        scratch.commit_all("init");
        make_executable(&scratch.join("run.sh"));
        assert_eq!(scratch.git_status(), " M run.sh\n");
        assert_matches_git(&scratch, "exec-bit");
    }

    #[cfg(unix)]
    #[test]
    fn worktree_symlink_target_change_matches_git() {
        let scratch = Scratch::new();
        scratch.write("target-one", "one\n");
        scratch.write("target-two", "two\n");
        std::os::unix::fs::symlink("target-one", scratch.join("link")).expect("symlink");
        scratch.commit_all("init");
        scratch.remove("link");
        std::os::unix::fs::symlink("target-two", scratch.join("link")).expect("symlink");

        // 同为 symlink（120000），只是指向变了 —— mg 用内容哈希判 M。
        assert_eq!(scratch.git_status(), " M link\n");
        assert_matches_git(&scratch, "symlink-target-change");
    }

    /// 本轮必做项：普通文件 ↔ symlink 必须是 `T`，而不是 `M`。
    #[cfg(unix)]
    #[test]
    fn worktree_type_change_regular_to_symlink_matches_git() {
        let scratch = Scratch::new();
        scratch.write("plain.txt", "plain\n");
        scratch.write("target", "target\n");
        scratch.commit_all("init");

        scratch.remove("plain.txt");
        std::os::unix::fs::symlink("target", scratch.join("plain.txt")).expect("symlink");
        assert_eq!(scratch.git_status(), " T plain.txt\n");
        assert_matches_git(&scratch, "regular-to-symlink");
    }

    /// 内容撞巧相同的类型变化（原 blob 的字节 == symlink 指向字符串）仍是 `T`：
    /// git 只看 mode 的类型位，不做内容比较。
    #[cfg(unix)]
    #[test]
    fn worktree_type_change_with_identical_bytes_matches_git() {
        let scratch = Scratch::new();
        scratch.write("same.txt", "target"); // 无结尾换行
        scratch.write("target", "x\n");
        scratch.commit_all("init");

        scratch.remove("same.txt");
        std::os::unix::fs::symlink("target", scratch.join("same.txt")).expect("symlink");
        // symlink 的 blob 内容就是指向字符串，与原普通文件的内容逐字节相同 ——
        // 也就是说这个场景里 index 与工作区的 oid 一致，只有类型不同。
        assert_eq!(scratch.git_status(), " T same.txt\n");
        assert_matches_git(&scratch, "type-change-identical-bytes");
    }

    #[cfg(unix)]
    #[test]
    fn worktree_type_change_symlink_to_regular_matches_git() {
        let scratch = Scratch::new();
        scratch.write("target", "target\n");
        std::os::unix::fs::symlink("target", scratch.join("link")).expect("symlink");
        scratch.commit_all("init");

        scratch.remove("link");
        scratch.write("link", "target"); // 内容 == 原 symlink 的 blob
        assert_eq!(scratch.git_status(), " T link\n");
        assert_matches_git(&scratch, "symlink-to-regular");
    }

    /// `X` 列的类型变化：HEAD 是普通文件、index 是 symlink（`git add` 之后）。
    #[cfg(unix)]
    #[test]
    fn staged_type_change_matches_git() {
        let scratch = Scratch::new();
        scratch.write("f", "plain\n");
        scratch.write("other", "other\n");
        scratch.commit_all("init");

        scratch.git_ok(&["rm", "-q", "--cached", "f"]);
        scratch.remove("f");
        std::os::unix::fs::symlink("other", scratch.join("f")).expect("symlink");
        scratch.git_ok(&["add", "f"]);

        assert_eq!(scratch.git_status(), "T  f\n");
        assert_matches_git(&scratch, "staged-type-change");
    }

    /// 类型变化 + 内容也变（`M` 在 `X`、`T` 在 `Y`）时两列必须各自正确。
    #[cfg(unix)]
    #[test]
    fn staged_modification_plus_worktree_type_change_matches_git() {
        let scratch = Scratch::new();
        scratch.write("f", "plain\n");
        scratch.write("other", "other\n");
        scratch.commit_all("init");

        scratch.write("f", "changed\n");
        scratch.git_ok(&["add", "f"]);
        scratch.remove("f");
        std::os::unix::fs::symlink("other", scratch.join("f")).expect("symlink");

        assert_eq!(scratch.git_status(), "MT f\n");
        assert_matches_git(&scratch, "staged-mod-plus-type-change");
    }

    #[test]
    fn ignored_files_and_rescue_match_git() {
        let scratch = Scratch::new();
        scratch.write(".gitignore", "*.log\n!keep.log\nbuild/\n");
        scratch.write("keep.log", "keep\n");
        scratch.write("drop.log", "drop\n");
        scratch.write("build/out.txt", "out\n");
        scratch.write("build/deep/out.txt", "deep\n");
        scratch.write("visible.txt", "v\n");
        assert_matches_git(&scratch, "ignored-and-rescued");
    }

    #[test]
    fn tracked_file_inside_ignored_directory_matches_git() {
        let scratch = Scratch::new();
        scratch.write("build/tracked.txt", "tracked\n");
        scratch.commit_all("init");
        scratch.write(".gitignore", "build/\n");
        scratch.git_ok(&["add", ".gitignore"]);
        scratch.git_ok(&["commit", "-q", "-m", "ignore build"]);
        scratch.write("build/tracked.txt", "changed\n");
        scratch.write("build/untracked.txt", "new\n");

        // git 只报已跟踪文件的改动；被忽略目录里的新文件不出现。
        assert_matches_git(&scratch, "tracked-inside-ignored-dir");
    }

    #[test]
    fn partial_untracked_directory_folding_matches_git() {
        let scratch = Scratch::new();
        scratch.write("tracked/keep.txt", "keep\n");
        scratch.commit_all("init");
        scratch.write("tracked/new.txt", "new\n");
        scratch.write("tracked/deep/a.txt", "a\n");
        scratch.write("tracked/deep/b.txt", "b\n");
        scratch.write("sibling.txt", "s\n");
        scratch.write("alpha/beta/gamma/deep.txt", "deep\n");
        scratch.write("d.txt", "d\n");
        scratch.write("d/y.txt", "y\n");
        assert_matches_git(&scratch, "folding");
    }

    #[test]
    fn staged_deletion_with_file_still_present_matches_git() {
        let scratch = Scratch::new();
        scratch.write("z/f.txt", "z\n");
        scratch.write("stays.txt", "s\n");
        scratch.commit_all("init");
        scratch.git_ok(&["rm", "-q", "--cached", "z/f.txt"]);
        assert_matches_git(&scratch, "rm-cached");
    }

    #[test]
    fn merge_conflict_matches_git() {
        let scratch = Scratch::new();
        scratch.write("conflict.txt", "base\n");
        scratch.write("clean.txt", "clean\n");
        scratch.commit_all("base");
        scratch.git_ok(&["checkout", "-q", "-b", "side"]);
        scratch.write("conflict.txt", "side\n");
        scratch.commit_all("side");
        scratch.git_ok(&["checkout", "-q", "main"]);
        scratch.write("conflict.txt", "main\n");
        scratch.commit_all("main");
        let merge = scratch.git(&["merge", "side"]);
        assert!(!merge.status.success(), "merge must conflict");

        assert!(scratch.git_status().contains("UU conflict.txt"), "sanity");
        assert_matches_git(&scratch, "merge-conflict");
    }

    #[cfg(unix)]
    #[test]
    fn staged_mode_change_matches_git() {
        let scratch = Scratch::new();
        scratch.write("run.sh", "#!/bin/sh\n");
        scratch.commit_all("init");
        make_executable(&scratch.join("run.sh"));
        scratch.git_ok(&["add", "run.sh"]);
        assert_eq!(scratch.git_status(), "M  run.sh\n");
        assert_matches_git(&scratch, "staged-mode-change");
    }

    #[test]
    fn nested_gitignore_scopes_match_git() {
        let scratch = Scratch::new();
        scratch.write("sub/keep.txt", "keep\n");
        scratch.write("sub/deep/keep.txt", "deep\n");
        scratch.write("sub/other.txt", "other\n");
        scratch.commit_all("init");
        // 子目录里的 `.gitignore`：`/keep.txt` 相对 sub 锚定。
        scratch.write("sub/.gitignore", "/keep.txt\n");
        scratch.write("root.tmp", "t\n");
        scratch.write(".gitignore", "*.tmp\n");
        assert_matches_git(&scratch, "nested-gitignore");
    }

    #[test]
    fn untracked_dir_with_ignored_contents_matches_git() {
        let scratch = Scratch::new();
        scratch.write("tracked.txt", "tracked\n");
        scratch.commit_all("init");
        scratch.write(".gitignore", "*.log\n");
        scratch.write("u/a.txt", "a\n");
        scratch.write("u/b.log", "b\n");
        scratch.write("only-ignored/c.log", "c\n");
        assert_matches_git(&scratch, "untracked-with-ignored");
    }

    #[test]
    fn modify_delete_conflict_matches_git() {
        let scratch = Scratch::new();
        scratch.write("doomed.txt", "base\n");
        scratch.write("anchor.txt", "anchor\n");
        scratch.commit_all("base");
        scratch.git_ok(&["checkout", "-q", "-b", "side"]);
        scratch.write("doomed.txt", "changed by them\n");
        scratch.commit_all("side modifies");
        scratch.git_ok(&["checkout", "-q", "main"]);
        scratch.git_ok(&["rm", "-q", "doomed.txt"]);
        scratch.git_ok(&["commit", "-q", "-m", "main deletes"]);
        let merge = scratch.git(&["merge", "side"]);
        assert!(!merge.status.success(), "merge must conflict");

        assert!(
            scratch.git_status().contains("DU doomed.txt"),
            "sanity: {}",
            scratch.git_status()
        );
        assert_matches_git(&scratch, "modify-delete-conflict");
    }

    #[test]
    fn add_add_conflict_matches_git() {
        let scratch = Scratch::new();
        scratch.write("anchor.txt", "anchor\n");
        scratch.commit_all("base");
        scratch.git_ok(&["checkout", "-q", "-b", "side"]);
        scratch.write("both.txt", "side\n");
        scratch.commit_all("side adds");
        scratch.git_ok(&["checkout", "-q", "main"]);
        scratch.write("both.txt", "main\n");
        scratch.commit_all("main adds");
        let merge = scratch.git(&["merge", "side"]);
        assert!(!merge.status.success(), "merge must conflict");

        assert!(
            scratch.git_status().contains("AA both.txt"),
            "sanity: {}",
            scratch.git_status()
        );
        assert_matches_git(&scratch, "add-add-conflict");
    }

    #[test]
    fn file_replaced_by_directory_matches_git() {
        let scratch = Scratch::new();
        scratch.write("f.txt", "a\n");
        scratch.commit_all("init");
        scratch.remove("f.txt");
        scratch.write("f.txt/inner.txt", "inner\n");
        assert_eq!(scratch.git_status(), " D f.txt\n");
        assert_matches_git(&scratch, "file-replaced-by-dir");
    }

    #[test]
    fn directory_replaced_by_file_matches_git() {
        let scratch = Scratch::new();
        scratch.write("dir/file.txt", "x\n");
        scratch.commit_all("init");
        std::fs::remove_dir_all(scratch.join("dir")).expect("remove dir");
        scratch.write("dir", "now a plain file\n");
        assert_eq!(scratch.git_status(), " D dir/file.txt\n?? dir\n");
        assert_matches_git(&scratch, "dir-replaced-by-file");
    }

    #[test]
    fn quoted_paths_match_git() {
        let scratch = Scratch::new();
        scratch.write("sp ace.txt", "space\n");
        scratch.write("\u{4e2d}\u{6587}.txt", "cjk\n");
        scratch.write("quo\"te.txt", "quote\n");
        scratch.write("tab\tname.txt", "tab\n");
        scratch.write("sp dir/inner.txt", "inner\n");
        assert_matches_git(&scratch, "quoted-paths");
    }

    #[test]
    fn tricky_name_ordering_matches_git() {
        let scratch = Scratch::new();
        scratch.write("foo.txt", "foo\n");
        scratch.write("foo/inside.txt", "inside\n");
        scratch.write("a-b.txt", "dash\n");
        scratch.write("a.b.txt", "dot\n");
        scratch.commit_all("init");
        scratch.write("foo.txt", "changed\n");
        scratch.write("foo/inside.txt", "changed\n");
        scratch.write("d.txt", "d\n");
        scratch.write("d/x.txt", "x\n");
        scratch.write("d-2.txt", "dash2\n");
        assert_matches_git(&scratch, "tricky-names");
    }

    #[test]
    fn deletion_in_index_and_missing_worktree_matches_git() {
        let scratch = Scratch::new();
        scratch.write("a.txt", "a\n");
        scratch.write("b.txt", "b\n");
        scratch.write("sub/c.txt", "c\n");
        scratch.commit_all("init");
        scratch.remove("a.txt");
        scratch.git_ok(&["add", "-A"]);
        scratch.write("b.txt", "changed\n");
        assert_matches_git(&scratch, "deletion");
    }
}
