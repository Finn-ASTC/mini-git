//! `mg branch` —— **T11（omp）**。
//!
//! * 无参数 / `-l [pattern]` → 列本地分支（`refs/heads/*`），当前分支前缀 `* `，
//!   短名与 `git branch --list --format='%(refname:short)'` 的集合一致（detached 时不打星号，
//!   也不打印 `(HEAD detached at …)` 这种伪条目）。
//! * `mg branch <name> [start]` → 在 `start`（缺省 `HEAD`）上建 `refs/heads/<name>`，**已存在则拒绝**。
//! * `mg branch -d <name>` → 先判「已合并」（`merge_base` 就是现成的），否则报
//!   `the branch '<name>' is not fully merged`；当前分支不能删（git: `cannot delete branch … used by worktree`）。
//! * `mg branch -m <old> <new>` / `mg branch -m <new>`（重命名当前分支）→ 改 `refs/heads/*`
//!   并维护 HEAD 的 symbolic ref。

use crate::cli::open_repo;
use crate::error::{Error, Result};
use crate::merge;
use crate::refs::{RefStore, HEADS_PREFIX};
use crate::repo::Repo;
use crate::worktree::materialize::{branch_oid, full_ref, head_commit, resolve_commit};

pub fn run(
    delete: bool,
    rename: bool,
    list: bool,
    name: Option<&str>,
    start: Option<&str>,
) -> Result<()> {
    let repo = open_repo()?;
    let refs = RefStore::new(&repo);

    if delete {
        let name = require_name(name, "-d")?;
        return delete_branch(&repo, &refs, name);
    }
    if rename {
        let target = require_name(name, "-m")?;
        return rename_branch(&repo, &refs, target, start);
    }
    if list {
        return list_branches(&refs, name);
    }
    match name {
        Some(name) => create_branch(&repo, &refs, name, start),
        None => list_branches(&refs, None),
    }
}

fn require_name<'a>(name: Option<&'a str>, flag: &str) -> Result<&'a str> {
    name.ok_or_else(|| Error::Other(format!("branch name required for `{flag}`")))
}

/// 列分支：短名升序（`RefStore::branches` 已按 refname 字节序），当前分支打 `* `。
fn list_branches(refs: &RefStore<'_>, pattern: Option<&str>) -> Result<()> {
    let current = current_branch_in(refs)?;
    let mut out = String::new();
    for branch in refs.branches()? {
        if let Some(pattern) = pattern {
            if !glob_match(pattern.as_bytes(), branch.as_bytes()) {
                continue;
            }
        }
        let marker = if current.as_deref() == Some(branch.as_str()) {
            "* "
        } else {
            "  "
        };
        out.push_str(marker);
        out.push_str(&branch);
        out.push('\n');
    }
    print!("{out}");
    Ok(())
}

fn current_branch_in(refs: &RefStore<'_>) -> Result<Option<String>> {
    match refs.read_head() {
        Ok(crate::refs::Head::Attached(name)) => Ok(Some(
            name.strip_prefix(HEADS_PREFIX).unwrap_or(name.as_str()).to_string(),
        )),
        Ok(crate::refs::Head::Detached(_)) => Ok(None),
        Err(Error::RefNotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

/// `mg branch <name> [start]`：只建引用，不切工作区（与 git 一致）。
fn create_branch(
    repo: &Repo,
    refs: &RefStore<'_>,
    name: &str,
    start: Option<&str>,
) -> Result<()> {
    validate_branch_name(name)?;
    let full = full_ref(name);
    if refs.exists(&full) {
        return Err(branch_exists(name));
    }
    let commit = resolve_commit(repo, start.unwrap_or("HEAD"))?;
    refs.update(&full, commit, None)
}

/// `mg branch -d <name>`：必须已合并进 HEAD，且不能是当前分支。
fn delete_branch(repo: &Repo, refs: &RefStore<'_>, name: &str) -> Result<()> {
    let full = full_ref(name);
    let Some(target) = branch_oid(repo, name)? else {
        return Err(Error::RefNotFound(full));
    };
    if current_branch_in(refs)?.as_deref() == Some(name) {
        return Err(Error::Other(format!(
            "cannot delete branch '{name}' used by worktree"
        )));
    }
    // 「已合并」= 目标分支是 HEAD 的祖先，用现成的 merge_base 判断（无需 --is-ancestor）。
    let merged = match head_commit(repo)? {
        Some(head) => merge::merge_base(repo, head, target)?.is_some_and(|base| base == target),
        None => false,
    };
    if !merged {
        return Err(Error::Other(format!(
            "the branch '{name}' is not fully merged"
        )));
    }
    refs.delete(&full)?;
    println!("Deleted branch {name} (was {}).", short_hex(target));
    Ok(())
}

/// `mg branch -m <old> <new>`；`start` 缺省表示「重命名当前分支」。
fn rename_branch(
    repo: &Repo,
    refs: &RefStore<'_>,
    target: &str,
    start: Option<&str>,
) -> Result<()> {
    let (old_name, new_name) = match start {
        Some(new_name) => (target.to_string(), new_name.to_string()),
        None => {
            let current = current_branch_in(refs)?.ok_or_else(|| {
                Error::Other("HEAD is detached: cannot rename the current branch".to_string())
            })?;
            (current, target.to_string())
        }
    };
    validate_branch_name(&new_name)?;
    let old_full = full_ref(&old_name);
    let new_full = full_ref(&new_name);
    let Some(oid) = branch_oid(repo, &old_name)? else {
        return Err(Error::Other(format!("no branch named '{old_name}'")));
    };
    if refs.exists(&new_full) {
        return Err(branch_exists(&new_name));
    }
    // 先建新引用、再改 HEAD、最后删旧引用：中途失败也不会出现「HEAD 指向不存在的分支」。
    refs.update(&new_full, oid, None)?;
    if current_branch_in(refs)?.as_deref() == Some(old_name.as_str()) {
        refs.set_head_attached(&new_name)?;
    }
    refs.delete(&old_full)
}

fn branch_exists(name: &str) -> Error {
    Error::Other(format!("a branch named '{name}' already exists"))
}

fn short_hex(oid: crate::oid::Oid) -> String {
    oid.to_hex()[..7].to_string()
}

/// `git check-ref-format` 的子集：拒绝会让引用文件变成怪东西的名字。
pub(crate) fn validate_branch_name(name: &str) -> Result<()> {
    let component_bad = name
        .split('/')
        .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"));
    let bad = name.is_empty()
        || name == "@"
        || name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("//")
        || name.contains("@{")
        || component_bad
        || name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c));
    if bad {
        return Err(Error::Other(format!("'{name}' is not a valid branch name")));
    }
    Ok(())
}

/// `git branch --list <pattern>` 的 wildmatch 子集：`*` / `?` / `[...]` / `\` 转义。
///
/// 与 git 一样**不做路径分隔符特判**（实测 `git branch --list 'feat*'` 会命中 `feature/x`），
/// 字符类支持 `!` / `^` 取反与 `a-z` 区间。
fn glob_match(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => {
            let mut skip = 0;
            loop {
                if glob_match(rest, &name[skip..]) {
                    return true;
                }
                if skip >= name.len() {
                    return false;
                }
                skip += 1;
            }
        }
        Some((b'?', rest)) => !name.is_empty() && glob_match(rest, &name[1..]),
        Some((b'[', rest)) => match class_end(rest) {
            Some(end) => match name.split_first() {
                Some((byte, tail)) => {
                    class_matches(&rest[..end], *byte) && glob_match(&rest[end + 1..], tail)
                }
                None => false,
            },
            // 未闭合的 `[` 按字面量处理（不 panic、不误判）。
            None => name.first() == Some(&b'[') && glob_match(rest, &name[1..]),
        },
        Some((b'\\', rest)) => match rest.split_first() {
            Some((escaped, tail)) => {
                name.first() == Some(escaped) && glob_match(tail, &name[1..])
            }
            None => name.first() == Some(&b'\\') && glob_match(&[], &name[1..]),
        },
        Some((byte, rest)) => name.first() == Some(byte) && glob_match(rest, &name[1..]),
    }
}

/// `[...]` 里 `]` 的下标（**不含**开头的 `[`）；未闭合返回 `None`。
fn class_end(rest: &[u8]) -> Option<usize> {
    let mut index = 0;
    if matches!(rest.first(), Some(b'!') | Some(b'^')) {
        index += 1;
    }
    if rest.get(index) == Some(&b']') {
        index += 1; // 首位的 `]` 是字面量
    }
    while index < rest.len() {
        if rest[index] == b']' {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn class_matches(class: &[u8], byte: u8) -> bool {
    let (negated, mut index) = match class.first() {
        Some(b'!') | Some(b'^') => (true, 1),
        _ => (false, 0),
    };
    let mut hit = false;
    while index < class.len() {
        if index + 2 < class.len() && class[index + 1] == b'-' {
            if class[index] <= byte && byte <= class[index + 2] {
                hit = true;
            }
            index += 3;
        } else {
            if class[index] == byte {
                hit = true;
            }
            index += 1;
        }
    }
    hit != negated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_name_validation_matches_ref_format_rules() {
        for ok in ["main", "feature/x", "v1.0", "a_b-c.d"] {
            assert!(validate_branch_name(ok).is_ok(), "{ok} should be valid");
        }
        for bad in [
            "", "a..b", "a b", "-x", "a/", "/a", "a.", "a//b", "a.lock", ".hidden", "a@{b}",
            "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\\b",
        ] {
            assert!(validate_branch_name(bad).is_err(), "{bad} should be rejected");
        }
    }

    /// 真值来自真实 `git branch --list <pattern>`（不硬编码任何期望集合）。
    #[test]
    fn glob_matches_real_git_branch_list() {
        use std::process::Command;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(root)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", root)
                .env("LC_ALL", "C")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@e")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@e")
                .output()
                .expect("run git")
        };
        if !git(&["--version"]).status.success() {
            return;
        }
        assert!(git(&["init", "-q", "--initial-branch=main"]).status.success());
        assert!(git(&["commit", "-q", "--allow-empty", "-m", "base"]).status.success());
        let names = ["feat-z", "feature/x", "main", "other/feat"];
        for name in ["feat-z", "feature/x", "other/feat"] {
            assert!(git(&["branch", name]).status.success());
        }

        for pattern in [
            "feat*", "*x", "feature/*", "*/*", "?eat*", "other?feat", "*", "[fo]*", "[!fo]*",
            "FEAT*", "feat\\*", "main", "*z",
        ] {
            let out = git(&["branch", "--list", pattern, "--format=%(refname:short)"]);
            assert!(out.status.success(), "git branch --list {pattern} failed");
            let want: Vec<String> = String::from_utf8(out.stdout)
                .expect("utf8")
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let got: Vec<String> = names
                .iter()
                .filter(|name| glob_match(pattern.as_bytes(), name.as_bytes()))
                .map(|name| name.to_string())
                .collect();
            assert_eq!(got, want, "pattern {pattern:?}");
        }
    }
}
