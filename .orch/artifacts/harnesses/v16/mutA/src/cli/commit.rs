//! `mg commit` —— **T12（hermes）**。
//!
//! 做的事：
//! 1. index（stage 0）→ **递归建 tree**：子目录先写，条目交给 `Tree::encode_payload`
//!    按 git 的规范序排序（`tree.rs`），所以 mg 写出的 tree oid 与 `git write-tree` 相同；
//! 2. parent = HEAD；`.git/MERGE_HEAD` 存在时把它作为**第二 parent**，提交成功后删掉
//!    `MERGE_HEAD` / `MERGE_MSG`（这样真实 `git merge` 留下的冲突状态能被 `mg commit` 收尾）；
//! 3. 无改动且没有 `--allow-empty` → 拒绝；index 里有 stage 1/2/3（未解决冲突）→ 拒绝；
//! 4. 写 commit 对象（author/committer 取 `user.name` / `user.email`，带可解释兜底；
//!    `--author` 只覆盖 author；时间格式与 git 一致：`<epoch> ±HHMM`）；
//! 5. 分支引用用 CAS 更新（detached HEAD 用 `set_head_detached`）；`--amend` 重写 HEAD，
//!    **保留原提交的 parent 链与 author**，丢掉原 HEAD 本身；
//! 6. `-a`：先把已跟踪文件的改动（含删除）写进 index，再提交（等价 `add -u`）；
//! 7. 提交后把 index 的 cache-tree 更新成新 tree —— 不更新的话真实 git 会拿过期的
//!    cache-tree 当证据，`git write-tree` / `git commit` 会提交**旧 tree**（见 `cli/add.rs` 的实测记录）。
//!
//! 已知限制：
//! * 不会打开编辑器：`-m` 缺失时优先用 `.git/MERGE_MSG`，都没有就报错（git 会起 `$EDITOR`）；
//! * 时间戳固定为「当前时刻 + 本机时区」，没有 `--date` / `GIT_AUTHOR_DATE` 之类的覆盖开关；
//! * `--author` 只接受 `Name <email>` 形式（与 git 的 `--author` 说明一致）。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::index::{Index, IndexEntry, StatData};
use crate::object::{Commit, FileMode, Kind, Signature, Tree, TreeEntry};
use crate::odb::Odb;
use crate::oid::Oid;
use crate::refs::{Head, RefStore, HEADS_PREFIX};
use crate::repo::Repo;
use crate::worktree::Worktree;

use super::add::invalidate_tree_cache;

pub fn run(
    message: Option<&str>,
    all: bool,
    amend: bool,
    allow_empty: bool,
    author: Option<&str>,
) -> Result<()> {
    let repo = crate::cli::open_repo()?;
    let odb = Odb::new(&repo);
    let store = RefStore::new(&repo);
    let worktree = Worktree::new(&repo);
    let mut index = Index::read(&repo)?;

    if index.has_conflicts() {
        return Err(Error::Other(
            "committing is not possible because you have unmerged files.".to_string(),
        ));
    }

    let head_oid = head_oid(&store)?;
    let head_commit = match head_oid {
        Some(oid) => Some(odb.read_object(oid)?.into_commit()?),
        None => None,
    };
    if amend && head_commit.is_none() {
        return Err(Error::Other("you have nothing to amend.".to_string()));
    }

    // `-a`：等价 `add -u`（已跟踪文件的改动与删除进 index）。
    if all {
        stage_tracked_changes(&repo, &worktree, &odb, &mut index)?;
    }

    let tree_oid = build_tree(&odb, &index)?;

    let merge_heads = read_merge_heads(&repo)?;
    let mut parents: Vec<Oid> = Vec::new();
    if amend {
        parents.extend(
            head_commit
                .as_ref()
                .map(|commit| commit.parents.clone())
                .unwrap_or_default(),
        );
    } else {
        if let Some(oid) = head_oid {
            parents.push(oid);
        }
        parents.extend(merge_heads.iter().copied());
    }

    let head_tree = head_commit
        .as_ref()
        .map(|commit| commit.tree)
        .unwrap_or_else(empty_tree_oid);
    if !allow_empty && !amend && merge_heads.is_empty() && tree_oid == head_tree {
        return Err(Error::Other(
            "nothing to commit, working tree clean".to_string(),
        ));
    }

    let text = match message {
        Some(text) => text.to_string(),
        None => match read_merge_msg(&repo)? {
            Some(text) => text,
            None => {
                return Err(Error::Other(
                    "no commit message given and mini-git does not open an editor (use -m)".to_string(),
                ))
            }
        },
    };
    // 消息 cleanup：与 git 的 `strbuf_stripspace` 一致 —— 去掉首尾空行、行尾空白、
    // 折叠连续空行。实测（git 2.55）`-m` 的 `#` 行**会保留**，`.git/MERGE_MSG`
    // 里的 `# Conflicts:` 也原样进提交（`git commit --no-edit` 收尾 merge 时验证过），
    // 所以这里**不**做注释行剥离。
    let cleaned = cleanup_message(&text);
    if cleaned.is_empty() {
        return Err(Error::Other(
            "aborting commit due to empty commit message".to_string(),
        ));
    }

    let when = now_unix();
    let tz = local_timezone_offset();
    let committer = identity(&repo).to_signature(when, &tz);
    let author = match author {
        Some(text) => parse_author(text, when, &tz)?,
        None => match head_commit {
            Some(commit) if amend => commit.author,
            _ => committer.clone(),
        },
    };

    let summary = String::from_utf8_lossy(
        cleaned
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default(),
    )
    .into_owned();

    let commit = Commit {
        tree: tree_oid,
        parents,
        author,
        committer,
        message: cleaned,
        extra_headers: Vec::new(),
    };
    let new_oid = odb.write(Kind::Commit, &commit.encode_payload())?;

    match store.read_head()? {
        Head::Attached(name) => store.update(&name, new_oid, Some(head_oid))?,
        Head::Detached(_) => store.set_head_detached(new_oid)?,
    }

    // 收尾 merge：真正的 git 也是提交后删这两个文件。
    if !merge_heads.is_empty() {
        let _ = std::fs::remove_file(repo.git_dir().join("MERGE_HEAD"));
        let _ = std::fs::remove_file(repo.git_dir().join("MERGE_MSG"));
    }

    // cache-tree 指向新 tree（与 git 的 `cache_tree_update` 等价）。
    index.tree_oid = Some(tree_oid);
    index.write(&repo)?;

    let label = match store.read_head()? {
        Head::Attached(name) => name
            .strip_prefix(HEADS_PREFIX)
            .unwrap_or(name.as_str())
            .to_string(),
        Head::Detached(_) => "(no branch)".to_string(),
    };
    println!("[{label} {}] {summary}", short_hex(new_oid, 7));
    Ok(())
}

// --------------------------------------------------------------------- tree 构建

/// `index → tree`：目录先写子树的 oid，再写父 tree；排序交给 `Tree::encode_payload`。
fn build_tree(odb: &Odb<'_>, index: &Index) -> Result<Oid> {
    let mut root: BTreeMap<Vec<u8>, Node> = BTreeMap::new();
    for entry in index.entries.iter().filter(|entry| entry.is_stage0()) {
        let components: Vec<&[u8]> = entry.path.split(|byte| *byte == b'/').collect();
        let Some((leaf, dirs)) = components.split_last() else {
            continue;
        };
        let mut node = &mut root;
        for dir in dirs {
            node = match node
                .entry(dir.to_vec())
                .or_insert_with(|| Node::Dir(BTreeMap::new()))
            {
                Node::Dir(children) => children,
                Node::File(..) => {
                    return Err(Error::corrupt(
                        "index",
                        format!(
                            "path {} has a file as its ancestor directory",
                            String::from_utf8_lossy(&entry.path)
                        ),
                    ))
                }
            };
        }
        node.insert(leaf.to_vec(), Node::File(entry.mode, entry.oid));
    }
    write_tree(odb, &root)
}

enum Node {
    File(FileMode, Oid),
    Dir(BTreeMap<Vec<u8>, Node>),
}

fn write_tree(odb: &Odb<'_>, entries: &BTreeMap<Vec<u8>, Node>) -> Result<Oid> {
    let mut tree = Vec::with_capacity(entries.len());
    for (name, node) in entries {
        let entry = match node {
            Node::File(mode, oid) => TreeEntry {
                mode: *mode,
                name: name.clone(),
                oid: *oid,
            },
            Node::Dir(children) => TreeEntry {
                mode: FileMode::Tree,
                name: name.clone(),
                oid: write_tree(odb, children)?,
            },
        };
        tree.push(entry);
    }
    // `encode_payload` 内部按 git 的 `base_name_compare` 排序，产出规范序。
    odb.write(Kind::Tree, &Tree::new(tree).encode_payload())
}

fn empty_tree_oid() -> Oid {
    Oid::hash_object("tree", b"")
}

// ------------------------------------------------------------------- `-a` 的暂存

/// `add -u`：已跟踪文件的内容/mode 变化进 index，工作区里没了的条目从 index 删除。
fn stage_tracked_changes(
    repo: &Repo,
    worktree: &Worktree<'_>,
    odb: &Odb<'_>,
    index: &mut Index,
) -> Result<()> {
    let mut tree_changed = false;
    let mut paths: Vec<Vec<u8>> = index
        .entries
        .iter()
        .filter(|entry| entry.is_stage0())
        .map(|entry| entry.path.clone())
        .collect();
    paths.sort();
    paths.dedup();

    let now_secs = now_unix() as u32;
    for path in paths {
        let abs = repo.work_path(&path)?;
        let meta = std::fs::symlink_metadata(&abs);
        match meta {
            Err(_) => {
                index.remove_all_stages(&path);
                tree_changed = true;
            }
            Ok(meta) if meta.is_dir() => {
                index.remove_all_stages(&path);
                tree_changed = true;
            }
            Ok(meta) => {
                let content = worktree.read_worktree_entry(&path)?;
                let oid = odb.write(Kind::Blob, &content.bytes)?;
                if index.lookup(&path).map(|entry| (entry.oid, entry.mode))
                    != Some((oid, content.mode))
                {
                    tree_changed = true;
                }
                let mut stat = StatData::from_metadata(&meta);
                if stat.mtime_s == now_secs {
                    stat.size = 0;
                }
                let mut entry = IndexEntry::new(path, oid, content.mode);
                entry.stat = stat;
                index.upsert(entry);
            }
        }
    }
    if tree_changed {
        invalidate_tree_cache(index);
    }
    Ok(())
}

// -------------------------------------------------------------------- merge 状态

fn read_merge_heads(repo: &Repo) -> Result<Vec<Oid>> {
    let path = repo.git_dir().join("MERGE_HEAD");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        out.push(Oid::from_hex(line).map_err(|err| {
            Error::corrupt("MERGE_HEAD", format!("not an object id: {line:?} ({err})"))
        })?);
    }
    Ok(out)
}

fn read_merge_msg(repo: &Repo) -> Result<Option<String>> {
    match std::fs::read_to_string(repo.git_dir().join("MERGE_MSG")) {
        Ok(text) => Ok(Some(text)),
        Err(_) => Ok(None),
    }
}

// ---------------------------------------------------------------- 身份 / 时间

/// 提交身份（`user.name` / `user.email` + 可解释兜底）。
#[derive(Debug, Clone)]
pub(crate) struct Identity {
    pub name: String,
    pub email: String,
}

impl Identity {
    pub(crate) fn to_signature(&self, when: i64, tz: &str) -> Signature {
        Signature::new(self.name.clone(), self.email.clone(), when, tz)
    }
}

/// `user.name` / `user.email`；缺省用 `$USER`（或 `mini-git`）与 `<user>@<hostname>`，
/// 与 git 的「从 passwd 与 hostname 拼」兜底同义（git 拼不出来时会直接报错，mg 用固定值）。
pub(crate) fn identity(repo: &Repo) -> Identity {
    let config = repo.config();
    let user = std::env::var("USER")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let name = config
        .get("user.name")
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| user.clone())
        .unwrap_or_else(|| "mini-git".to_string());
    let email = config
        .get("user.email")
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{}@{}", user.unwrap_or_else(|| name.clone()), hostname()));
    Identity { name, email }
}

fn hostname() -> String {
    if let Ok(value) = std::env::var("HOSTNAME") {
        if !value.trim().is_empty() {
            return value;
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .map(|text| text.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// `--author "Name <email>"`（git 也只接受这一种写法）。
fn parse_author(text: &str, when: i64, tz: &str) -> Result<Signature> {
    let open = text.find('<');
    let close = text.find('>');
    let (Some(open), Some(close)) = (open, close) else {
        return Err(Error::Other(format!(
            "--author {text:?} is not in the 'Name <email>' form"
        )));
    };
    if close < open {
        return Err(Error::Other(format!(
            "--author {text:?} is not in the 'Name <email>' form"
        )));
    }
    let name = text[..open].trim();
    let email = text[open + 1..close].trim();
    if email.is_empty() {
        return Err(Error::Other(format!(
            "--author {text:?} has an empty email"
        )));
    }
    Ok(Signature::new(name, email, when, tz))
}

pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs() as i64)
        .unwrap_or(0)
}

/// 本机时区的 `±HHMM` 字符串（提交/标签的签名行格式与 git 完全一致）。
pub(crate) fn local_timezone_offset() -> String {
    let seconds = local_utc_offset(now_unix());
    let sign = if seconds < 0 { '-' } else { '+' };
    let abs = seconds.abs();
    format!("{sign}{:02}{:02}", abs / 3600, (abs % 3600) / 60)
}

/// 本机相对 UTC 的偏移（秒，东为正）。优先 `TZ`，其次 `/etc/localtime`（TZif 解析）。
pub(crate) fn local_utc_offset(now: i64) -> i32 {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim();
        if !tz.is_empty() {
            if let Some(offset) = parse_fixed_offset(tz) {
                return offset;
            }
            let name = tz.trim_start_matches(':');
            let path = if name.starts_with('/') {
                std::path::PathBuf::from(name)
            } else {
                Path::new("/usr/share/zoneinfo").join(name)
            };
            if let Some(offset) = read_tzif_offset(&path, now) {
                return offset;
            }
        }
    }
    read_tzif_offset(Path::new("/etc/localtime"), now).unwrap_or(0)
}

/// `UTC` / `GMT` / `Z` / `+08:00` / `-0500` 这类固定偏移写法。
fn parse_fixed_offset(tz: &str) -> Option<i32> {
    let text = tz.trim();
    if text.eq_ignore_ascii_case("utc")
        || text.eq_ignore_ascii_case("gmt")
        || text.eq_ignore_ascii_case("z")
    {
        return Some(0);
    }
    let (sign, rest) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    let (hours, minutes) = match digits.len() {
        1 | 2 => (digits.parse::<i32>().ok()?, 0),
        3 => (digits[..1].parse::<i32>().ok()?, digits[1..].parse::<i32>().ok()?),
        4 => (
            digits[..2].parse::<i32>().ok()?,
            digits[2..].parse::<i32>().ok()?,
        ),
        _ => return None,
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3600 + minutes * 60))
}

/// 解析 TZif（v1/v2/v3/v4），返回 `now` 时刻生效的 UTC 偏移（秒）。
fn read_tzif_offset(path: &Path, now: i64) -> Option<i32> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 44 || &data[..4] != b"TZif" {
        return None;
    }
    let version = data[4];
    let counts = |at: usize| -> Option<[usize; 6]> {
        let mut out = [0usize; 6];
        for (index, slot) in out.iter_mut().enumerate() {
            let chunk = data.get(at + index * 4..at + index * 4 + 4)?;
            *slot = u32::from_be_bytes(chunk.try_into().ok()?) as usize;
        }
        Some(out)
    };

    if matches!(version, b'2' | b'3' | b'4') {
        // 第一块是 32 位数据，只为定位第二个 header。
        let [_gmt1, _std1, leap1, time1, type1, chars1] = counts(20)?;
        let mut cursor = 44 + time1 * 5 + type1 * 6 + chars1 + leap1 * 8 + _std1 + _gmt1;
        if data.get(cursor..cursor + 4)? != b"TZif" {
            return None;
        }
        cursor += 20;
        let [_, _, _, timecnt, typecnt, _] = counts(cursor)?;
        return parse_timezone_block(&data, cursor + 24, timecnt, typecnt, 8, now);
    }

    let [_gmt1, _std1, _leap1, timecnt, typecnt, _chars1] = counts(20)?;
    parse_timezone_block(&data, 44, timecnt, typecnt, 4, now)
}

/// 时间转换表 + `ttinfo` 记录（`gmtoff` 4 字节 + `isdst` + `abbrind`）。
fn parse_timezone_block(
    data: &[u8],
    start: usize,
    timecnt: usize,
    typecnt: usize,
    width: usize,
    now: i64,
) -> Option<i32> {
    let types_start = start + timecnt * width + timecnt;
    let mut chosen: Option<usize> = None;
    for index in 0..timecnt {
        let chunk = data.get(start + index * width..start + (index + 1) * width)?;
        let when = if width == 8 {
            i64::from_be_bytes(chunk.try_into().ok()?)
        } else {
            i32::from_be_bytes(chunk.try_into().ok()?) as i64
        };
        if when > now {
            break;
        }
        let kind = usize::from(*data.get(start + timecnt * width + index)?);
        if kind < typecnt {
            chosen = Some(kind);
        }
    }
    // 第一段转换之前：用第一个「非夏令时」的记录（TZif 的惯例）。
    let index = chosen.or_else(|| {
        (0..typecnt).find(|candidate| data.get(types_start + candidate * 6 + 4) == Some(&0))
    })?;
    let record = data.get(types_start + index * 6..types_start + index * 6 + 4)?;
    Some(i32::from_be_bytes(record.try_into().ok()?))
}

// -------------------------------------------------------------------- 消息清理

/// git 的 `strbuf_stripspace`（comment 字符传 0）：删首尾空行、行尾空白、折叠连续空行。
/// 提交与 annotated tag 的消息共用（两处都实测过：`#` 行**不**被删）。
pub(crate) fn cleanup_message(text: &str) -> Vec<u8> {
    let mut lines: Vec<&str> = Vec::new();
    for raw in text.split('\n') {
        lines.push(raw.trim_end());
    }
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    for line in lines {
        if line.is_empty() && out.last().is_some_and(|previous| previous.is_empty()) {
            continue;
        }
        out.push(line);
    }
    if out.is_empty() {
        return Vec::new();
    }
    let mut text = out.join("\n");
    text.push('\n');
    text.into_bytes()
}

fn short_hex(oid: Oid, len: usize) -> String {
    oid.to_hex()[..len].to_string()
}

fn head_oid(store: &RefStore<'_>) -> Result<Option<Oid>> {
    match store.resolve("HEAD") {
        Ok(oid) => Ok(Some(oid)),
        Err(Error::RefNotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Output};

    /// 真值来自真实 `git` 进程（时区格式、消息 cleanup、`commit-tree` 的逐字节等价）。
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

    fn git_text(dir: &Path, args: &[&str]) -> String {
        String::from_utf8_lossy(&git(dir, args).stdout).trim().to_string()
    }

    #[test]
    fn cleanup_message_matches_git_stripspace() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(git(dir, &["init", "-q", "-b", "main"]).status.success());
        std::fs::write(dir.join("f"), b"f\n").unwrap();
        assert!(git(dir, &["add", "-A"]).status.success());

        for raw in [
            "subject",
            "subject\n\nbody line\n",
            "\n\nsubject\n\n\nbody\n\n\n",
            "# leading comment kept\nsubject   \n\n\nbody\n",
            "subject with trailing spaces   ",
        ] {
            assert!(
                git(dir, &["commit", "-q", "--allow-empty", "-m", raw])
                    .status
                    .success()
            );
            let payload = git(dir, &["cat-file", "commit", "HEAD"]);
            let text = String::from_utf8_lossy(&payload.stdout).into_owned();
            let want = text
                .split_once("\n\n")
                .map(|(_, body)| body.to_string())
                .expect("commit payload has a blank line");
            let got = String::from_utf8(cleanup_message(raw)).unwrap();
            assert_eq!(got, want, "cleanup mismatch for input {raw:?}");
        }
    }

    #[test]
    fn timezone_offset_matches_system_date() {
        if let Ok(tz) = std::env::var("TZ") {
            let tz = tz.trim().trim_start_matches(':');
            let known = tz.is_empty()
                || parse_fixed_offset(tz).is_some()
                || tz.starts_with('/')
                || Path::new("/usr/share/zoneinfo").join(tz).exists();
            if !known {
                // POSIX TZ 字符串（例如 `EST5EDT`）不支持，跳过。
                return;
            }
        }
        let Ok(out) = Command::new("date").arg("+%z").output() else {
            return;
        };
        if !out.status.success() {
            return;
        }
        let want = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if want.len() != 5 {
            return;
        }
        assert_eq!(local_timezone_offset(), want, "TZ offset must match `date +%z`");
    }

    #[test]
    fn fixed_offsets_parse() {
        assert_eq!(parse_fixed_offset("+08:00"), Some(8 * 3600));
        assert_eq!(parse_fixed_offset("-0500"), Some(-5 * 3600));
        assert_eq!(parse_fixed_offset("UTC"), Some(0));
        assert_eq!(parse_fixed_offset("+0530"), Some(5 * 3600 + 1800));
        assert_eq!(parse_fixed_offset("Asia/Shanghai"), None);
    }

    #[test]
    fn merge_heads_and_merge_msg_are_read() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init(tmp.path(), "main").unwrap();
        let oid = Oid::hash_object("blob", b"x");
        std::fs::write(
            repo.git_dir().join("MERGE_HEAD"),
            format!("{}\n", oid.to_hex()),
        )
        .unwrap();
        std::fs::write(repo.git_dir().join("MERGE_MSG"), "merge them\n").unwrap();

        assert_eq!(read_merge_heads(&repo).unwrap(), vec![oid]);
        assert_eq!(
            read_merge_msg(&repo).unwrap().as_deref(),
            Some("merge them\n")
        );

        std::fs::write(repo.git_dir().join("MERGE_HEAD"), "not-an-oid\n").unwrap();
        assert!(read_merge_heads(&repo).is_err());
    }

    #[test]
    fn author_requires_the_standard_form() {
        let signature = parse_author("A U Thor <a@example.com>", 1, "+0800").unwrap();
        assert_eq!(signature, Signature::new("A U Thor", "a@example.com", 1, "+0800"));
        assert!(parse_author("no email here", 1, "+0800").is_err());
        assert!(parse_author("Name <>", 1, "+0800").is_err());
    }

    #[test]
    fn tree_building_matches_git_write_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(git(dir, &["init", "-q", "-b", "main"]).status.success());
        std::fs::create_dir_all(dir.join("foo")).unwrap();
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("foo.txt"), b"foo\n").unwrap();
        std::fs::write(dir.join("foo/b.txt"), b"b\n").unwrap();
        std::fs::write(dir.join("a/b/c.txt"), b"c\n").unwrap();
        std::fs::write(dir.join("run.sh"), b"#!/bin/sh\n").unwrap();
        assert!(git(dir, &["add", "-A"]).status.success());

        let repo = Repo::discover(dir).unwrap();
        let odb = Odb::new(&repo);
        let index = Index::read(&repo).unwrap();
        let mine = build_tree(&odb, &index).unwrap().to_hex();
        assert_eq!(mine, git_text(dir, &["write-tree"]));
    }
}
