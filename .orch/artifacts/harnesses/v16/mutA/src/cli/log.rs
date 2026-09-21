//! `mg log` —— **T12（hermes）**。
//!
//! 遍历顺序 = git 的默认顺序（提交时间倒序、同刻按**入队顺序**稳定、首父先入队）：
//! 用一个「按 committer date 插入」的队列 —— 与 git 的
//! `commit_list_insert_by_date()` 等价（新条目插在**所有 date >= 新 date** 的条目之后），
//! 这样同时间戳的分叉提交顺序也与 git 一致。
//!
//! `--oneline` = 短 oid(7) + `%s`（subject = 第一段，按 git 的 `format_subject`：
//! 段落内多行用单个空格连接、行内前导空白保留、遇到空行即停）。
//!
//! 已知限制：
//! * 短 oid 固定 7 位，不做 git 的「歧义时加长」；
//! * `--graph` / `--format` / `--follow` / 日期过滤等不在本轮签名内；
//! * 非 `--oneline` 的「medium」格式是**尽力复刻**（commit/Author/Date/Merge/缩进消息），
//!   本轮逐字节对拍的硬标准只针对 `--oneline`。

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::object::{Commit, Object};
use crate::odb::Odb;
use crate::oid::Oid;
use crate::refs::{Head, RefStore, HEADS_PREFIX};

pub fn run(oneline: bool, max_count: Option<usize>, rev: Option<&str>) -> Result<()> {
    let repo = crate::cli::open_repo()?;
    let odb = Odb::new(&repo);
    let store = RefStore::new(&repo);

    let start = match rev {
        Some(rev) => store.resolve(rev)?,
        None => store.resolve("HEAD").map_err(|err| match err {
            Error::RefNotFound(_) => Error::Other(format!(
                "your current branch '{}' does not have any commits yet",
                current_branch(&store)
            )),
            other => other,
        })?,
    };

    let mut cache: HashMap<Oid, Commit> = HashMap::new();
    let mut queue: Vec<(i64, Oid)> = Vec::new();
    let mut seen: Vec<Oid> = vec![start];
    push_by_date(&mut queue, &odb, &mut cache, start)?;

    let limit = max_count.unwrap_or(usize::MAX);
    let mut printed = 0usize;
    while printed < limit {
        let Some((_date, oid)) = queue.first().copied() else {
            break;
        };
        queue.remove(0);
        let commit = load_commit(&odb, &mut cache, oid)?;
        if oneline {
            println!("{} {}", short_hex(oid, 7), subject(&commit));
        } else {
            // git 的 medium 格式用**空行分隔**相邻条目（末尾不留空行）。
            if printed > 0 {
                println!();
            }
            print_medium(oid, &commit);
        }
        printed += 1;
        for parent in &commit.parents {
            if seen.contains(parent) {
                continue;
            }
            seen.push(*parent);
            push_by_date(&mut queue, &odb, &mut cache, *parent)?;
        }
    }
    Ok(())
}

/// git 的 `commit_list_insert_by_date`：插在最后一个 `date >= 新 date` 的条目之后。
fn push_by_date(
    queue: &mut Vec<(i64, Oid)>,
    odb: &Odb<'_>,
    cache: &mut HashMap<Oid, Commit>,
    oid: Oid,
) -> Result<()> {
    let date = load_commit(odb, cache, oid)?.committer.when;
    let at = queue
        .iter()
        .position(|(queued, _)| *queued < date)
        .unwrap_or(queue.len());
    queue.insert(at, (date, oid));
    Ok(())
}

fn load_commit(
    odb: &Odb<'_>,
    cache: &mut HashMap<Oid, Commit>,
    oid: Oid,
) -> Result<Commit> {
    if let Some(commit) = cache.get(&oid) {
        return Ok(commit.clone());
    }
    let commit = match odb.read_object(oid)? {
        Object::Commit(commit) => commit,
        other => {
            return Err(Error::Other(format!(
                "{oid} is a {}, not a commit",
                other.kind()
            )))
        }
    };
    cache.insert(oid, commit.clone());
    Ok(commit)
}

/// git 的 `%s`：第一段（到第一个空行为止）里的各行用单个空格连接，
/// 行内前导空白保留、行尾 `\n` 去掉；首行就是空行时 subject 为空。
fn subject(commit: &Commit) -> String {
    let mut out = String::new();
    let mut first = true;
    for line in commit.message.split(|byte| *byte == b'\n') {
        let line = String::from_utf8_lossy(line);
        if line.trim().is_empty() {
            break;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(&line);
        first = false;
    }
    out
}

fn print_medium(oid: Oid, commit: &Commit) {
    println!("commit {}", oid.to_hex());
    if commit.parents.len() > 1 {
        let parents: Vec<String> = commit
            .parents
            .iter()
            .map(|parent| short_hex(*parent, 7))
            .collect();
        println!("Merge: {}", parents.join(" "));
    }
    // medium 格式的 Author 行只写 `Name <email>`（没有时间戳），时间在 Date 行。
    println!(
        "Author: {} <{}>",
        String::from_utf8_lossy(&commit.author.name),
        String::from_utf8_lossy(&commit.author.email)
    );
    println!(
        "Date:   {}",
        format_date(commit.author.when, &commit.author.tz)
    );
    println!();
    // 实测 git 的 medium 格式：消息每行（含空行）都缩进 4 空格，消息末尾的换行不产生
    // 额外的空行；下一条 `commit` 行紧接着上一条消息的最后一行。
    let message = commit
        .message
        .strip_suffix(b"\n")
        .unwrap_or(&commit.message);
    for line in message.split(|byte| *byte == b'\n') {
        println!("    {}", String::from_utf8_lossy(line));
    }
}

/// `Mon Sep 17 00:00:00 2001 +0800`（提交自带的时区；day-of-month 空格补齐）。
fn format_date(when: i64, tz: &str) -> String {
    let offset = parse_tz_offset(tz);
    let local = when + i64::from(offset);
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let weekday = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
        [((days + 4).rem_euclid(7)) as usize];
    let month_name = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(month - 1) as usize];
    format!(
        "{weekday} {month_name} {day:2} {:02}:{:02}:{:02} {year} {tz}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
    )
}

fn parse_tz_offset(tz: &str) -> i32 {
    let bytes = tz.as_bytes();
    if bytes.len() != 5 {
        return 0;
    }
    let sign = match bytes[0] {
        b'-' => -1,
        _ => 1,
    };
    let digits: String = tz[1..].chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 4 {
        return 0;
    }
    let hours: i32 = digits[..2].parse().unwrap_or(0);
    let minutes: i32 = digits[2..].parse().unwrap_or(0);
    sign * (hours * 3600 + minutes * 60)
}

/// Howard Hinnant 的 `civil_from_days`：儒略日 → (year, month, day)。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn current_branch(store: &RefStore<'_>) -> String {
    match store.read_head() {
        Ok(Head::Attached(name)) => name
            .strip_prefix(HEADS_PREFIX)
            .unwrap_or(name.as_str())
            .to_string(),
        Ok(Head::Detached(oid)) => oid.to_hex(),
        Err(_) => "HEAD".to_string(),
    }
}

fn short_hex(oid: Oid, len: usize) -> String {
    oid.to_hex()[..len].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::Repo;
    use std::path::Path;
    use std::process::{Command, Output};

    fn git(dir: &Path, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "A U Thor")
            .env("GIT_AUTHOR_EMAIL", "a@example.com")
            .env("GIT_COMMITTER_NAME", "A U Thor")
            .env("GIT_COMMITTER_EMAIL", "a@example.com")
            .env("GIT_AUTHOR_DATE", "1700000000 +0800")
            .env("GIT_COMMITTER_DATE", "1700000000 +0800")
            .output()
            .expect("run git")
    }

    #[test]
    fn subject_joins_the_first_paragraph_like_git() {
        let mut commit = Commit {
            tree: Oid::zeros(),
            parents: Vec::new(),
            author: crate::object::Signature::new("a", "a@b", 0, "+0000"),
            committer: crate::object::Signature::new("a", "a@b", 0, "+0000"),
            message: b"subject line one\ncontinued line two\n\nbody here\n".to_vec(),
            extra_headers: Vec::new(),
        };
        assert_eq!(subject(&commit), "subject line one continued line two");

        commit.message = b"one line\n".to_vec();
        assert_eq!(subject(&commit), "one line");

        // 前导空白保留（git 实测：`   indented\nsecond` → `   indented second`）。
        commit.message = b"   indented\nsecond\n\nbody\n".to_vec();
        assert_eq!(subject(&commit), "   indented second");

        // 第一段就是空行 → 空 subject。
        commit.message = b"\nleading blank\n".to_vec();
        assert_eq!(subject(&commit), "");
    }

    #[test]
    fn date_format_matches_git_default_layout() {
        // `git log` 默认格式的 Date 列：`Wed Nov 15 06:13:20 2023 +0800`
        // （epoch 1700000000 +0800）。
        assert_eq!(
            format_date(1_700_000_000, "+0800"),
            "Wed Nov 15 06:13:20 2023 +0800"
        );
        assert_eq!(
            format_date(1_700_000_000, "+0000"),
            "Tue Nov 14 22:13:20 2023 +0000"
        );
        // 个位数的日：git 用空格补齐（`%e`）。
        assert_eq!(
            format_date(0, "+0000"),
            "Thu Jan  1 00:00:00 1970 +0000"
        );
    }

    #[test]
    fn oneline_order_matches_git_for_a_fork_and_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(git(dir, &["init", "-q", "-b", "main"]).status.success());
        let mut day = 1_700_000_000;
        // 每个提交写**不同的**文件，避免制造冲突（这里只考察遍历顺序）。
        let commit = |file: &str, message: &str, date: i64| {
            std::fs::write(dir.join(file), format!("{message}\n")).unwrap();
            assert!(git(dir, &["add", "-A"]).status.success());
            let out = Command::new("git")
                .args(["commit", "-q", "-m", message])
                .current_dir(dir)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("HOME", dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "A U Thor")
                .env("GIT_AUTHOR_EMAIL", "a@example.com")
                .env("GIT_COMMITTER_NAME", "A U Thor")
                .env("GIT_COMMITTER_EMAIL", "a@example.com")
                .env("GIT_AUTHOR_DATE", format!("{date} +0800"))
                .env("GIT_COMMITTER_DATE", format!("{date} +0800"))
                .output()
                .expect("run git");
            assert!(
                out.status.success(),
                "commit failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        commit("base.txt", "base", day);
        day += 100;
        assert!(git(dir, &["checkout", "-q", "-b", "side"]).status.success());
        commit("side.txt", "side", day);
        day += 100;
        assert!(git(dir, &["checkout", "-q", "main"]).status.success());
        commit("main.txt", "main", day);
        day += 100;
        let out = Command::new("git")
            .args(["merge", "--no-ff", "-m", "merge side", "side"])
            .current_dir(dir)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "A U Thor")
            .env("GIT_AUTHOR_EMAIL", "a@example.com")
            .env("GIT_COMMITTER_NAME", "A U Thor")
            .env("GIT_COMMITTER_EMAIL", "a@example.com")
            .env("GIT_AUTHOR_DATE", format!("{day} +0800"))
            .env("GIT_COMMITTER_DATE", format!("{day} +0800"))
            .output()
            .expect("run git");
        assert!(out.status.success(), "merge failed");

        let repo = Repo::discover(dir).unwrap();
        let odb = Odb::new(&repo);
        let store = RefStore::new(&repo);
        let mut got = String::new();
        let mut queue = Vec::new();
        let mut cache = HashMap::new();
        let mut seen = Vec::new();
        let start = store.resolve("HEAD").unwrap();
        seen.push(start);
        push_by_date(&mut queue, &odb, &mut cache, start).unwrap();
        while let Some((_, oid)) = queue.first().copied() {
            queue.remove(0);
            let entry = load_commit(&odb, &mut cache, oid).unwrap();
            got.push_str(&format!("{} {}\n", short_hex(oid, 7), subject(&entry)));
            for parent in &entry.parents {
                if seen.contains(parent) {
                    continue;
                }
                seen.push(*parent);
                push_by_date(&mut queue, &odb, &mut cache, *parent).unwrap();
            }
        }
        let want = String::from_utf8_lossy(&git(dir, &["log", "--oneline"]).stdout).into_owned();
        assert_eq!(got, want, "oneline order must match git's default traversal");
    }
}
