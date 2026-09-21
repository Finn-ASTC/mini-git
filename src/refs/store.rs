//! `RefStore` 的方法实现。**T2（hermes）实现范围**。
//!
//! 为什么单独一个文件：`mod.rs` 是 CONTROLLER-OWNED（冻结公共签名），
//! 实现体必须放在模块自己的文件里，这样 author agent 永远不需要改冻结文件。
//! 本文件的 `pub` 签名来自 `mod.rs` 的 `struct` 声明，**不允许改签名**。
//!
//! 磁盘语义与真实 git 一致：
//! * HEAD：`ref: <refname>\n`（attached）或 `<40 hex>\n`（detached）。
//! * loose ref：`<git_dir>/<refname>`，内容 `<40 hex>\n`。
//! * `packed-refs`：可选的 `#` 注释行 + `<oid> <refname>` 行，
//!   `^<peeled>` 行属于**上一条** ref（读时必须跳过）。
//! * 写入一律走 `<path>.lock`（`create_new`）+ `rename`，天然拒绝并发写者。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::oid::Oid;
use crate::repo::Repo;

use super::{Head, RefStore, HEADS_PREFIX, TAGS_PREFIX};

/// 符号引用链的最大跟随深度（防止 `refs/a -> refs/b -> refs/a` 这类环）。
const MAX_SYMREF_DEPTH: usize = 8;

impl<'a> RefStore<'a> {
    pub fn new(repo: &'a Repo) -> Self {
        RefStore { repo }
    }

    pub fn repo(&self) -> &'a Repo {
        self.repo
    }

    pub fn read_head(&self) -> Result<Head> {
        let path = self.repo.head_path();
        let Some(text) = read_text(&path)? else {
            return Err(Error::RefNotFound("HEAD".to_string()));
        };
        let trimmed = text.trim();
        if let Some(target) = trimmed.strip_prefix("ref:") {
            let target = target.trim();
            if target.is_empty() {
                return Err(Error::corrupt("HEAD", "empty symbolic ref target"));
            }
            return Ok(Head::Attached(target.to_string()));
        }
        Oid::from_hex(trimmed).map(Head::Detached).map_err(|e| {
            Error::corrupt(
                "HEAD",
                format!("expected `ref: <refname>` or a 40-hex object id: {e}"),
            )
        })
    }

    /// 解析任意 rev 写法到 oid（v1 支持：`HEAD`、完整 40 位 hex、`refs/...`、短分支名、tag 名）。
    pub fn resolve(&self, name: &str) -> Result<Oid> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::RefNotFound(name.to_string()));
        }
        if name == "HEAD" {
            return match self.read_head()? {
                Head::Detached(oid) => Ok(oid),
                Head::Attached(target) => self
                    .lookup(&target)?
                    .ok_or_else(|| Error::RefNotFound(name.to_string())),
            };
        }
        // 完整 40 位 hex：直接当 oid，**不要求对象存在**。
        if let Ok(oid) = Oid::from_hex(name) {
            return Ok(oid);
        }
        // 含 `/` 的按全名找。
        if name.contains('/') {
            return self
                .lookup(name)?
                .ok_or_else(|| Error::RefNotFound(name.to_string()));
        }
        // 短名：先 refs/heads/<name>，再 refs/tags/<name>。
        for candidate in [
            format!("{HEADS_PREFIX}{name}"),
            format!("{TAGS_PREFIX}{name}"),
        ] {
            if let Some(oid) = self.lookup(&candidate)? {
                return Ok(oid);
            }
        }
        Err(Error::RefNotFound(name.to_string()))
    }

    pub fn exists(&self, name: &str) -> bool {
        if fs::symlink_metadata(self.ref_path(name)).is_ok() {
            return true;
        }
        match self.packed() {
            Ok(entries) => entries.iter().any(|(packed_name, _)| packed_name == name),
            Err(_) => false,
        }
    }

    /// 全部引用（loose ∪ packed，loose 覆盖 packed），按名字排序。
    pub fn list(&self) -> Result<Vec<(String, Oid)>> {
        // BTreeMap 的迭代顺序就是 refname 的字节序。
        let mut merged: BTreeMap<String, Oid> = self.packed()?.into_iter().collect();
        let refs_dir = self.refs_dir();
        let mut loose = Vec::new();
        collect_loose_refs(self.repo.git_dir(), &refs_dir, &mut loose)?;
        for (name, _path) in loose {
            if let Some(oid) = self.lookup(&name)? {
                merged.insert(name, oid);
            }
        }
        Ok(merged.into_iter().collect())
    }

    /// 仅 packed-refs。
    pub fn packed(&self) -> Result<Vec<(String, Oid)>> {
        let Some(text) = read_text(&self.packed_refs_path())? else {
            return Ok(Vec::new());
        };
        parse_packed_refs(&text)
    }

    /// 分支名（`refs/heads/` 下的短名）。
    pub fn branches(&self) -> Result<Vec<String>> {
        let all: Vec<String> = self.list()?.into_iter().map(|(name, _)| name).collect();
        let mut out = Vec::new();
        for name in &all {
            if name.starts_with(HEADS_PREFIX) {
                out.push(short_ref_name(name, &all));
            }
        }
        Ok(out)
    }

    /// 原子更新引用；`expected` 为 `None` 表示「必须不存在」（创建），
    /// `Some(oid)` 表示 CAS（当前值不等则 `Error::RefConflict`）。
    pub fn update(&self, name: &str, new: Oid, expected: Option<Option<Oid>>) -> Result<()> {
        let path = self.ref_path(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock = lock_path(&path);
        let wanted = expected_label(&expected);
        // 先拿锁再比对旧值：否则「检查 → 写入」之间仍能被别的写者插队。
        let file = acquire_lock(&lock, name, &wanted)?;
        let current = match self.lookup(name) {
            Ok(current) => current,
            Err(err) => {
                release_lock(&lock);
                return Err(err);
            }
        };
        let verdict = match (expected.flatten(), current) {
            (None, None) => Ok(()),
            (None, Some(actual)) => Err(conflict(name, &wanted, &actual.to_hex())),
            (Some(expect), Some(actual)) if expect == actual => Ok(()),
            (Some(expect), actual) => Err(conflict(
                name,
                &expect.to_hex(),
                &actual.map_or_else(|| MISSING.to_string(), |oid| oid.to_hex()),
            )),
        };
        if let Err(err) = verdict {
            drop(file);
            release_lock(&lock);
            return Err(err);
        }
        let body = format!("{}\n", new.to_hex());
        commit_locked(file, &lock, &path, body.as_bytes())
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let path = self.ref_path(name);
        let loose = fs::symlink_metadata(&path).is_ok();
        let packed = self
            .packed()?
            .iter()
            .any(|(packed_name, _)| packed_name == name);
        if !loose && !packed {
            return Err(Error::RefNotFound(name.to_string()));
        }
        if loose {
            fs::remove_file(&path)?;
            prune_empty_dirs(&path, &self.refs_dir());
        }
        if packed {
            // 只删 loose 的话，packed-refs 里的同名条目会把 ref「复活」。
            self.drop_packed_entry(name)?;
        }
        Ok(())
    }

    pub fn set_head_attached(&self, branch: &str) -> Result<()> {
        let branch = branch.trim();
        if branch.is_empty() {
            return Err(Error::RefNotFound(branch.to_string()));
        }
        // 短名与全名都接受，落盘一律是全名。
        let full = if branch.starts_with("refs/") {
            branch.to_string()
        } else {
            format!("{HEADS_PREFIX}{branch}")
        };
        let path = self.repo.head_path();
        let lock = lock_path(&path);
        let file = acquire_lock(&lock, "HEAD", "write symbolic ref")?;
        commit_locked(file, &lock, &path, format!("ref: {full}\n").as_bytes())
    }

    pub fn set_head_detached(&self, oid: Oid) -> Result<()> {
        let path = self.repo.head_path();
        let lock = lock_path(&path);
        let file = acquire_lock(&lock, "HEAD", "write detached head")?;
        commit_locked(file, &lock, &path, format!("{}\n", oid.to_hex()).as_bytes())
    }

    /// 把 refname 解析成 oid：loose 优先，其次 packed-refs；不存在返回 `Ok(None)`。
    fn lookup(&self, name: &str) -> Result<Option<Oid>> {
        self.lookup_depth(name, 0)
    }

    fn lookup_depth(&self, name: &str, depth: usize) -> Result<Option<Oid>> {
        if depth > MAX_SYMREF_DEPTH {
            return Err(Error::corrupt(
                name.to_string(),
                "symbolic ref chain too deep (cycle?)",
            ));
        }
        let path = self.ref_path(name);
        let Some(text) = read_text(&path)? else {
            return Ok(self
                .packed()?
                .into_iter()
                .find(|(packed_name, _)| packed_name == name)
                .map(|(_, oid)| oid));
        };
        let trimmed = text.trim();
        if let Some(target) = trimmed.strip_prefix("ref:") {
            let target = target.trim();
            if target.is_empty() {
                return Err(Error::corrupt(
                    name.to_string(),
                    "empty symbolic ref target",
                ));
            }
            return self.lookup_depth(target, depth + 1);
        }
        Oid::from_hex(trimmed)
            .map(Some)
            .map_err(|e| Error::corrupt(name.to_string(), format!("not a 40-hex object id: {e}")))
    }

    /// 把 `packed-refs` 中的某个条目重写掉（其余条目与 `^peeled` 行原样保留）。
    fn drop_packed_entry(&self, name: &str) -> Result<()> {
        let path = self.packed_refs_path();
        let Some(text) = read_text(&path)? else {
            return Ok(());
        };
        let mut out = String::with_capacity(text.len());
        let mut skip_peeled = false;
        for line in text.lines() {
            if line.starts_with('^') {
                if skip_peeled {
                    continue;
                }
                out.push_str(line);
                out.push('\n');
                continue;
            }
            skip_peeled = false;
            if !line.starts_with('#') && !line.trim().is_empty() {
                if let Some((_, refname)) = line.split_once(' ') {
                    if refname.trim() == name {
                        skip_peeled = true;
                        continue;
                    }
                }
            }
            out.push_str(line);
            out.push('\n');
        }
        if out == text {
            return Ok(());
        }
        let lock = lock_path(&path);
        let file = acquire_lock(&lock, "packed-refs", "rewrite")?;
        commit_locked(file, &lock, &path, out.as_bytes())
    }

    fn ref_path(&self, name: &str) -> PathBuf {
        self.repo.git_dir().join(name)
    }

    fn refs_dir(&self) -> PathBuf {
        self.repo.git_dir().join("refs")
    }

    fn packed_refs_path(&self) -> PathBuf {
        self.repo.git_dir().join("packed-refs")
    }
}

const MISSING: &str = "ref does not exist";

fn conflict(name: &str, expected: &str, actual: &str) -> Error {
    Error::RefConflict {
        name: name.to_string(),
        expected: expected.to_string(),
        actual: actual.to_string(),
    }
}

fn expected_label(expected: &Option<Option<Oid>>) -> String {
    match expected {
        None | Some(None) => MISSING.to_string(),
        Some(Some(oid)) => oid.to_hex(),
    }
}

/// 读文本文件；不存在返回 `Ok(None)`，非 UTF-8 与其它 IO 错误按类型上报。
fn read_text(path: &Path) -> Result<Option<String>> {
    match fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| Error::corrupt(path.display().to_string(), "non-UTF-8 contents")),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io(e)),
    }
}

/// `<path>.lock`（`HEAD` → `HEAD.lock`，`packed-refs` → `packed-refs.lock`）。
fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    path.with_file_name(name)
}

fn acquire_lock(lock: &Path, name: &str, expected: &str) -> Result<File> {
    match OpenOptions::new().write(true).create_new(true).open(lock) {
        Ok(file) => Ok(file),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(conflict(
            name,
            expected,
            &format!("{} is held by another writer", lock.display()),
        )),
        Err(e) => Err(Error::Io(e)),
    }
}

fn release_lock(lock: &Path) {
    let _ = fs::remove_file(lock);
}

/// 写入 + `fsync` + `rename`；任何一步失败都清掉锁文件。
fn commit_locked(mut file: File, lock: &Path, path: &Path, body: &[u8]) -> Result<()> {
    if let Err(e) = file.write_all(body).and_then(|()| file.sync_all()) {
        drop(file);
        release_lock(lock);
        return Err(Error::Io(e));
    }
    drop(file);
    if let Err(e) = fs::rename(lock, path) {
        release_lock(lock);
        return Err(Error::Io(e));
    }
    Ok(())
}

/// 删除 ref 后顺手清掉空目录（真实 git 也会 `prune_ref_directory`）。
fn prune_empty_dirs(path: &Path, stop_at: &Path) {
    let mut cur = path.parent();
    while let Some(dir) = cur {
        if !dir.starts_with(stop_at) || dir == stop_at {
            break;
        }
        if fs::remove_dir(dir).is_err() {
            break;
        }
        cur = dir.parent();
    }
}

/// 递归收集 `refs/` 下的 loose ref（名字相对 `base`，即含 `refs/` 前缀）；
/// 跳过 lock 文件与 `.` 开头的名字。
fn collect_loose_refs(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir)? {
        entries.push(entry?.path());
    }
    entries.sort();
    for path in entries {
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if file_name.starts_with('.') || file_name.ends_with(".lock") {
            continue;
        }
        if path.is_dir() {
            collect_loose_refs(base, &path, out)?;
            continue;
        }
        if let Ok(rel) = path.strip_prefix(base) {
            if let Some(name) = rel.to_str() {
                out.push((name.replace('\\', "/"), path));
            }
        }
    }
    Ok(())
}

/// 解析 `packed-refs`：跳过注释行与 `^peeled` 行。
fn parse_packed_refs(text: &str) -> Result<Vec<(String, Oid)>> {
    let mut out = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let Some((oid_hex, refname)) = line.split_once(' ') else {
            return Err(Error::corrupt(
                "packed-refs",
                format!("line {}: unexpected format {line:?}", idx + 1),
            ));
        };
        let oid = Oid::from_hex(oid_hex).map_err(|e| {
            Error::corrupt(
                "packed-refs",
                format!("line {}: invalid object id: {e}", idx + 1),
            )
        })?;
        let refname = refname.trim();
        if refname.is_empty() {
            return Err(Error::corrupt(
                "packed-refs",
                format!("line {}: empty refname", idx + 1),
            ));
        }
        out.push((refname.to_string(), oid));
    }
    Ok(out)
}

/// `%(refname:short)` 的语义：依次尝试候选名，取第一个在全仓库范围内
/// **唯一可解析**的（等价于真实 git 的 `shorten_unambiguous_ref`）。
fn short_ref_name(name: &str, all: &[String]) -> String {
    for candidate in shorten_candidates(name) {
        let hits = all
            .iter()
            .filter(|other| refname_matches(other, &candidate))
            .count();
        if hits == 1 {
            return candidate;
        }
    }
    name.to_string()
}

/// 候选名按 `ref_rev_parse_rules` 的**反序**生成：越具体的命名空间越先试。
/// 例：`refs/heads/feature/x` → `feature/x` → `heads/feature/x` → 全名。
fn shorten_candidates(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(rest) = name.strip_prefix("refs/remotes/") {
        if let Some(middle) = rest.strip_suffix("/HEAD") {
            if !middle.is_empty() {
                out.push(middle.to_string());
            }
        }
        out.push(rest.to_string());
    } else if let Some(rest) = name.strip_prefix("refs/heads/") {
        out.push(rest.to_string());
    } else if let Some(rest) = name.strip_prefix("refs/tags/") {
        out.push(rest.to_string());
    }
    if let Some(rest) = name.strip_prefix("refs/") {
        out.push(rest.to_string());
    }
    out.push(name.to_string());
    out
}

/// 候选名能否解析到 `reference`（对应 `ref_rev_parse_rules` 的六条规则）。
fn refname_matches(reference: &str, candidate: &str) -> bool {
    const RULES: [(&str, &str); 6] = [
        ("", ""),
        ("refs/", ""),
        ("refs/tags/", ""),
        ("refs/heads/", ""),
        ("refs/remotes/", ""),
        ("refs/remotes/", "/HEAD"),
    ];
    RULES
        .iter()
        .any(|(prefix, suffix)| rule_matches(prefix, suffix, candidate, reference))
}

fn rule_matches(prefix: &str, suffix: &str, candidate: &str, reference: &str) -> bool {
    let mut expected = String::with_capacity(prefix.len() + candidate.len() + suffix.len());
    expected.push_str(prefix);
    expected.push_str(candidate);
    expected.push_str(suffix);
    reference == expected
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Output};

    // ---------------------------------------------------------------- git 测试床

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// 真值只能来自 git 二进制；环境一律隔离，避免用户/系统配置干扰。
    fn git_raw(dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "T2")
            .env("GIT_AUTHOR_EMAIL", "t2@example.com")
            .env("GIT_COMMITTER_NAME", "T2")
            .env("GIT_COMMITTER_EMAIL", "t2@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0800")
            .env("GIT_COMMITTER_DATE", "1700000000 +0800")
            .args(args)
            .output()
            .expect("failed to spawn git")
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = git_raw(dir, args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn git_fails(dir: &Path, args: &[&str]) -> bool {
        !git_raw(dir, args).status.success()
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("a.txt"), "hello\n").unwrap();
        git(dir, &["add", "a.txt"]);
        git(dir, &["commit", "-q", "-m", "initial"]);
    }

    fn store_for(dir: &Path) -> (Repo, RefStore<'static>) {
        // 见下方 `store_in`：RefStore 借用 Repo，测试里用泄漏的 Box 简化生命周期。
        let repo: &'static Repo = Box::leak(Box::new(Repo::discover(dir).unwrap()));
        (repo.clone(), RefStore::new(repo))
    }

    fn parse_show_ref(text: &str) -> Vec<(String, Oid)> {
        let mut out: Vec<(String, Oid)> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (hex, name) = line.split_once(' ').expect("`<oid> <refname>`");
                (
                    name.trim().to_string(),
                    Oid::from_hex(hex).expect("show-ref oid"),
                )
            })
            .collect();
        out.sort();
        out
    }

    fn loose_ref_text(dir: &Path, name: &str) -> String {
        fs::read_to_string(dir.join(".git").join(name)).unwrap()
    }

    // ------------------------------------------------------------------- 用例

    /// 验收 1：读真实 git 的仓库，`list()`/`branches()` 与 git 命令逐条相同。
    #[test]
    fn list_and_branches_match_real_git() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo(dir);
        git(dir, &["branch", "feature/x"]);
        git(dir, &["tag", "v1"]);
        git(dir, &["tag", "-a", "v2", "-m", "annotated"]);

        let (_repo, store) = store_for(dir);

        let mut got = store.list().unwrap();
        got.sort();
        assert_eq!(got, parse_show_ref(&git(dir, &["show-ref"])));

        let mut branches = store.branches().unwrap();
        branches.sort();
        let mut expected: Vec<String> = git(
            dir,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
        )
        .lines()
        .map(str::to_string)
        .collect();
        expected.sort();
        assert_eq!(branches, expected);
    }

    /// 验收 2：attached / detached HEAD 与 `git symbolic-ref`、`git rev-parse` 一致。
    #[test]
    fn head_attached_and_detached_match_git() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo(dir);
        let (_repo, store) = store_for(dir);

        assert_eq!(
            store.read_head().unwrap(),
            Head::Attached("refs/heads/main".to_string())
        );
        assert_eq!(git(dir, &["symbolic-ref", "HEAD"]), "refs/heads/main");

        let head = store.resolve("HEAD").unwrap();
        assert_eq!(head.to_hex(), git(dir, &["rev-parse", "HEAD"]));

        store.set_head_detached(head).unwrap();
        assert_eq!(store.read_head().unwrap(), Head::Detached(head));
        assert!(git_fails(dir, &["symbolic-ref", "HEAD"]));
        assert_eq!(git(dir, &["rev-parse", "HEAD"]), head.to_hex());

        // 短名与全名都要能落成 `ref: refs/heads/main`。
        store.set_head_attached("main").unwrap();
        assert_eq!(git(dir, &["symbolic-ref", "HEAD"]), "refs/heads/main");
        store.set_head_attached("refs/heads/main").unwrap();
        assert_eq!(git(dir, &["symbolic-ref", "HEAD"]), "refs/heads/main");
        assert_eq!(
            store.read_head().unwrap(),
            Head::Attached("refs/heads/main".to_string())
        );
    }

    /// HEAD 的两种错误形态：缺失 → RefNotFound，内容非法 → Corrupt。
    #[test]
    fn head_missing_and_garbage_are_typed_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let repo = Repo::init(dir, crate::repo::DEFAULT_INITIAL_BRANCH).unwrap();
        let store = RefStore::new(&repo);

        assert_eq!(
            store.read_head().unwrap(),
            Head::Attached("refs/heads/main".to_string())
        );
        // 未出生分支：HEAD 合法，但目标 ref 还不存在。
        assert!(matches!(store.resolve("HEAD"), Err(Error::RefNotFound(_))));

        fs::write(repo.head_path(), "garbage\n").unwrap();
        assert!(matches!(store.read_head(), Err(Error::Corrupt { .. })));

        fs::remove_file(repo.head_path()).unwrap();
        assert!(matches!(store.read_head(), Err(Error::RefNotFound(_))));
    }

    /// 验收 3：`git pack-refs --all` 之后 loose 文件消失，`list()` 仍需返回同样的集合。
    #[test]
    fn list_reads_packed_refs_after_pack_refs() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo(dir);
        git(dir, &["branch", "topic"]);
        git(dir, &["tag", "v1"]);
        git(dir, &["tag", "-a", "v2", "-m", "annotated"]);

        let (_repo, store) = store_for(dir);
        let before = store.list().unwrap();
        assert!(!before.is_empty());

        git(dir, &["pack-refs", "--all"]);
        assert!(
            !dir.join(".git/refs/heads/main").exists(),
            "loose ref should be gone"
        );
        assert!(
            !dir.join(".git/refs/tags/v1").exists(),
            "loose ref should be gone"
        );

        let packed = store.packed().unwrap();
        assert!(!packed.is_empty(), "packed() must see packed-refs");
        assert!(
            packed.iter().any(|(name, _)| name == "refs/heads/main"),
            "packed() is missing refs/heads/main: {packed:?}"
        );
        // `^peeled` 行不能变成独立 ref。
        let raw = fs::read_to_string(dir.join(".git/packed-refs")).unwrap();
        assert!(
            raw.lines().any(|line| line.starts_with('^')),
            "expected a peeled line in packed-refs:\n{raw}"
        );
        assert!(
            !packed.iter().any(|(name, _)| name.starts_with('^')),
            "peeled line leaked into packed(): {packed:?}"
        );

        let after = store.list().unwrap();
        assert_eq!(before, after, "list() changed after pack-refs");
        assert_eq!(after, parse_show_ref(&git(dir, &["show-ref"])));
        assert_eq!(
            store.branches().unwrap(),
            vec!["main".to_string(), "topic".to_string()]
        );
    }

    /// resolve 的 v1 子集：HEAD / 40 hex / 全名 / 短分支 / tag。
    #[test]
    fn resolve_supports_the_v1_forms() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo(dir);
        git(dir, &["tag", "v1"]);
        let (_repo, store) = store_for(dir);

        let head = store.resolve("HEAD").unwrap();
        assert_eq!(head.to_hex(), git(dir, &["rev-parse", "HEAD"]));
        assert_eq!(store.resolve("main").unwrap(), head);
        assert_eq!(store.resolve("refs/heads/main").unwrap(), head);
        assert_eq!(store.resolve(&head.to_hex()).unwrap(), head);
        assert_eq!(
            store.resolve("v1").unwrap().to_hex(),
            git(dir, &["rev-parse", "v1"])
        );

        // 完整 hex 不要求对象存在。
        let fake = Oid::from_hex("1111111111111111111111111111111111111111").unwrap();
        assert_eq!(store.resolve(&fake.to_hex()).unwrap(), fake);

        assert!(matches!(store.resolve("nope"), Err(Error::RefNotFound(_))));
        assert!(matches!(
            store.resolve("refs/heads/nope"),
            Err(Error::RefNotFound(_))
        ));
        assert!(matches!(store.resolve(""), Err(Error::RefNotFound(_))));
    }

    /// 验收 4：CAS 反例必须失败，且失败时磁盘上的 ref 一个字节都不能变。
    #[test]
    fn update_cas_failures_do_not_touch_disk() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo(dir);
        let (_repo, store) = store_for(dir);

        let head = store.resolve("HEAD").unwrap();
        let other = Oid::from_hex("2222222222222222222222222222222222222222").unwrap();
        let name = "refs/heads/new";
        let expected_text = format!("{}\n", head.to_hex());

        // 创建语义：expected=None 且 ref 不存在 → 成功。
        store.update(name, head, None).unwrap();
        assert!(store.exists(name));
        assert_eq!(loose_ref_text(dir, name), expected_text);
        assert_eq!(loose_ref_text(dir, "refs/heads/new"), format!("{head}\n"));

        // ref 已存在 + expected=None → 必须冲突，磁盘不变。
        let err = store.update(name, other, None).unwrap_err();
        assert!(matches!(err, Error::RefConflict { .. }), "got {err:?}");
        assert_eq!(loose_ref_text(dir, name), expected_text);

        // 旧值不匹配 → 必须冲突，磁盘不变。
        let err = store.update(name, other, Some(Some(other))).unwrap_err();
        assert!(matches!(err, Error::RefConflict { .. }), "got {err:?}");
        assert_eq!(loose_ref_text(dir, name), expected_text);

        // Some(None) 与 None 同义：要求不存在。
        let err = store.update(name, other, Some(None)).unwrap_err();
        assert!(matches!(err, Error::RefConflict { .. }), "got {err:?}");
        assert_eq!(loose_ref_text(dir, name), expected_text);

        // 旧值匹配 → 成功，并且 git 也认。
        store.update(name, other, Some(Some(head))).unwrap();
        assert_eq!(loose_ref_text(dir, name), format!("{other}\n"));
        assert_eq!(git(dir, &["rev-parse", name]), other.to_hex());

        // 并发写者留下的 lock 文件必须让写入失败（原子性的另一半）。
        let lock = dir.join(".git/refs/heads/new.lock");
        fs::write(&lock, "").unwrap();
        let err = store.update(name, head, Some(Some(other))).unwrap_err();
        assert!(matches!(err, Error::RefConflict { .. }), "got {err:?}");
        assert_eq!(loose_ref_text(dir, name), format!("{other}\n"));
        fs::remove_file(&lock).unwrap();
        assert!(!lock.exists(), "lock file must not survive a failed update");
    }

    /// 验收 5：packed 之后 delete 必须让 `git show-ref` 也看不到它。
    #[test]
    fn delete_removes_packed_entry_too() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo(dir);
        git(dir, &["branch", "x"]);
        let head = git(dir, &["rev-parse", "HEAD"]);
        let (_repo, store) = store_for(dir);

        assert!(store.exists("refs/heads/x"));
        git(dir, &["pack-refs", "--all"]);
        assert!(!dir.join(".git/refs/heads/x").exists());
        // 只有 packed 条目时也算「存在」。
        assert!(store.exists("refs/heads/x"));
        assert!(git(dir, &["show-ref"]).contains("refs/heads/x"));

        store.delete("refs/heads/x").unwrap();
        assert!(!store.exists("refs/heads/x"));
        assert!(
            !store
                .list()
                .unwrap()
                .iter()
                .any(|(name, _)| name == "refs/heads/x"),
            "packed entry survived delete()"
        );
        let refs = git(dir, &["show-ref"]);
        assert!(!refs.contains("refs/heads/x"), "git still sees it:\n{refs}");
        assert!(
            refs.contains("refs/heads/main"),
            "packed-refs got corrupted:\n{refs}"
        );
        assert_eq!(git(dir, &["rev-parse", "refs/heads/main"]), head);
        assert!(git_fails(dir, &["rev-parse", "--verify", "refs/heads/x"]));
        // 重写过的 packed-refs 必须仍是 git 认的合法文件。
        assert!(
            !git(dir, &["fsck", "--no-progress"]).contains("packed"),
            "git fsck complained about packed-refs"
        );

        // 已经不存在 → RefNotFound。
        assert!(matches!(
            store.delete("refs/heads/x"),
            Err(Error::RefNotFound(_))
        ));

        // loose 的普通删除路径。
        git(dir, &["branch", "y"]);
        assert!(dir.join(".git/refs/heads/y").exists());
        store.delete("refs/heads/y").unwrap();
        assert!(!dir.join(".git/refs/heads/y").exists());
        assert!(git_fails(dir, &["rev-parse", "--verify", "refs/heads/y"]));
    }

    /// 短名（`%(refname:short)`）规则的对拍：heads / tags / remotes 三类命名空间，
    /// 含各种歧义组合（同名 head+tag、嵌套名、`refs/remotes/<x>/HEAD`）。
    #[test]
    fn short_names_match_git_for_each_ref() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo(dir);
        let head = git(dir, &["rev-parse", "HEAD"]);
        for reference in [
            "refs/heads/feature/x",
            "refs/heads/a/b/c",
            "refs/heads/origin/main",
            "refs/heads/deep/x/y",
            "refs/tags/v1",
            "refs/tags/main",
            "refs/tags/feature/x",
            "refs/tags/y",
            "refs/remotes/origin/main",
            "refs/remotes/origin/HEAD",
            "refs/remotes/up/feature/x",
        ] {
            git(dir, &["update-ref", reference, &head]);
        }

        let (_repo, store) = store_for(dir);
        let all: Vec<String> = store.list().unwrap().into_iter().map(|(n, _)| n).collect();
        let oracle = git(
            dir,
            &["for-each-ref", "--format=%(refname) %(refname:short)"],
        );

        let mut checked = 0;
        for line in oracle.lines() {
            let (full, short) = line.split_once(' ').expect("`<refname> <short>`");
            assert!(
                all.iter().any(|name| name == full),
                "list() is missing {full}"
            );
            assert_eq!(
                short_ref_name(full, &all),
                short,
                "short name mismatch for {full}"
            );
            checked += 1;
        }
        assert!(checked >= 12, "oracle listed only {checked} refs");
        // 双向：list() 不能多出 oracle 里没有的条目（`^peeled` 行最容易漏进来）。
        assert_eq!(checked, all.len(), "list() != git for-each-ref: {all:?}");

        // 全部打包之后（loose 文件消失）对拍结果必须一模一样。
        git(dir, &["pack-refs", "--all"]);
        assert!(!dir.join(".git/refs/heads/main").exists());
        let repacked: Vec<String> = store.list().unwrap().into_iter().map(|(n, _)| n).collect();
        assert_eq!(repacked, all, "list() changed after pack-refs");
        assert_eq!(
            oracle,
            git(
                dir,
                &["for-each-ref", "--format=%(refname) %(refname:short)"]
            )
        );
        for line in oracle.lines() {
            let (full, short) = line.split_once(' ').expect("`<refname> <short>`");
            assert_eq!(
                short_ref_name(full, &all),
                short,
                "packed mismatch for {full}"
            );
        }
        let mut branches = store.branches().unwrap();
        branches.sort();
        let mut oracle_branches: Vec<String> = git(
            dir,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
        )
        .lines()
        .map(str::to_string)
        .collect();
        oracle_branches.sort();
        assert_eq!(branches, oracle_branches);
    }

    /// packed 覆盖 / loose 优先 / 同名排序。
    #[test]
    fn loose_shadows_packed_and_list_is_sorted() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        init_repo(dir);
        git(dir, &["branch", "packed-only"]);
        git(dir, &["pack-refs", "--all"]);

        let (_repo, store) = store_for(dir);
        let head = store.resolve("HEAD").unwrap();
        let other = Oid::from_hex("3333333333333333333333333333333333333333").unwrap();

        // ref 只存在于 packed-refs 时，loose 写入后自动取得优先级。
        assert!(store
            .packed()
            .unwrap()
            .iter()
            .any(|(n, _)| n == "refs/heads/main"));
        store
            .update("refs/heads/main", other, Some(Some(head)))
            .unwrap();
        assert_eq!(store.resolve("refs/heads/main").unwrap(), other);
        assert_eq!(
            store
                .list()
                .unwrap()
                .into_iter()
                .find(|(n, _)| n == "refs/heads/main")
                .map(|(_, oid)| oid),
            Some(other)
        );
        assert_eq!(git(dir, &["rev-parse", "refs/heads/main"]), other.to_hex());

        let names: Vec<String> = store.list().unwrap().into_iter().map(|(n, _)| n).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "list() must be sorted by refname");
    }
}
