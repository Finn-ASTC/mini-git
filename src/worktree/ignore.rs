//! `.gitignore` 匹配。**T4（codex）实现范围**。
//!
//! v1 支持的范围（其余显式报 Unsupported 或忽略，并在 README 声明）：
//! * 每个目录下的 `.gitignore`，规则作用于该目录及其子目录。
//! * `*` `?` `[...]` 通配、`!` 取反、前导 `/` 锚定、尾随 `/` 仅匹配目录。
//! * **不支持**：`.git/info/exclude`、global excludes、`**` 的完整语义、
//!   `core.excludesFile`、嵌套 `.gitignore` 的复杂交互。
//!
//! 语义要点（照 git 的可见行为实现，不发明新语义）：
//! * 规则按「祖先目录先、子目录后」的顺序累积，匹配时**最后一条命中的规则胜出**，
//!   所以 `!` 能把先被 `*.txt` 忽略的文件救回来。
//! * 未锚定的模式（其中不含 `/`）对路径的**每个 component** 逐个匹配 —— 这是
//!   git「逐层下降后拿 basename 比」的等价形式，因此 `build/` 或 `build` 这类
//!   目录规则能覆盖 `build/x/y.txt`。
//! * 锚定的模式（前导 `/`，或模式中含 `/`）从 `base` 起在每个 component 边界上
//!   逐前缀匹配，因此 `/a/b` 只命中 `a/b` 及其子树。
//! * `**` 按普通 `*` 处理（`*` 不跨越 `/`），不支持 git 的 `**/` 前导语义。
//! * 被 ignore 的目录**不被下降**（`load` 与 `scan` 都如此）：这也意味着被忽略
//!   目录内的 `.gitignore` / 反选规则不会生效 —— 与 git 一致（git 不列出被忽略
//!   目录的内容，其中的规则没有机会生效）。
//! * 解析永不 panic：没有闭合 `]` 的 `[...]` 按字面量 `[` 处理；未知转义按字面量
//!   处理；非 UTF-8 的 `.gitignore` 也不会导致失败。

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::repo::Repo;

#[derive(Debug, Default, Clone)]
pub struct Ignore {
    rules: Vec<IgnoreRule>,
}

#[derive(Debug, Clone)]
pub struct IgnoreRule {
    /// 规则所在目录（仓库相对，空表示根）。
    pub base: Vec<u8>,
    pub pattern: Vec<u8>,
    pub negated: bool,
    pub dir_only: bool,
    pub anchored: bool,
}

impl Ignore {
    /// 读取工作区里所有 `.gitignore`（根目录 + 各级子目录）。
    ///
    /// 被 ignore 的目录不会下降 —— 与 git 的扫描行为一致。bare 仓库返回空规则集。
    pub fn load(repo: &Repo) -> Result<Ignore> {
        let Some(workdir) = repo.workdir() else {
            return Ok(Ignore::default());
        };
        let mut rules = Vec::new();
        load_dir(workdir, &[], &mut rules)?;
        Ok(Ignore { rules })
    }

    pub fn from_rules(rules: Vec<IgnoreRule>) -> Self {
        Ignore { rules }
    }

    pub fn rules(&self) -> &[IgnoreRule] {
        &self.rules
    }

    /// `path` 为仓库相对 `/` 分隔路径；`is_dir` 影响 `dir_only` 规则。
    /// 最后一条匹配的规则胜出（因此 `!` 取反可以救回被忽略的文件）。
    pub fn is_ignored(&self, path: &[u8], is_dir: bool) -> bool {
        let mut ignored = false;
        for rule in &self.rules {
            if rule_matches(rule, path, is_dir) {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

/// 递归读取 `dir` 下的 `.gitignore`；`base` 是 `dir` 的仓库相对路径。
fn load_dir(dir: &Path, base: &[u8], rules: &mut Vec<IgnoreRule>) -> Result<()> {
    let gitignore = dir.join(".gitignore");
    if gitignore.is_file() {
        let bytes = std::fs::read(&gitignore)?;
        parse_gitignore(&bytes, base, rules);
    }

    let mut children: Vec<(Vec<u8>, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = os_bytes(&entry.file_name());
        if name == b".git" {
            continue;
        }
        // symlink 不跟随：链接到目录的 symlink 不会被当作目录下降。
        if !std::fs::symlink_metadata(entry.path())?.is_dir() {
            continue;
        }
        children.push((name, entry.path()));
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));

    let snapshot = Ignore {
        rules: rules.clone(),
    };
    for (name, path) in children {
        let child = join_rel(base, &name);
        if snapshot.is_ignored(&child, true) {
            continue;
        }
        load_dir(&path, &child, rules)?;
    }
    Ok(())
}

/// 解析一个 `.gitignore` 文件的字节；行序即优先级（后面的行更优先）。
fn parse_gitignore(bytes: &[u8], base: &[u8], rules: &mut Vec<IgnoreRule>) {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    for raw in bytes.split(|b| *b == b'\n') {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        let line = strip_trailing_spaces(raw);
        if line.is_empty() || line[0] == b'#' {
            continue;
        }

        let mut rest = line;
        let mut negated = false;
        if rest[0] == b'!' {
            negated = true;
            rest = &rest[1..];
        }
        if rest.is_empty() {
            continue;
        }

        let mut dir_only = false;
        if rest.last() == Some(&b'/') {
            dir_only = true;
            rest = &rest[..rest.len() - 1];
        }
        let mut anchored = false;
        if rest.first() == Some(&b'/') {
            anchored = true;
            rest = &rest[1..];
        }
        if rest.is_empty() {
            continue;
        }
        // 模式中出现 `/` 即相对 `base` 锚定（gitignore(5)：`doc/frotz` 等价于 `/doc/frotz`）。
        if rest.contains(&b'/') {
            anchored = true;
        }

        rules.push(IgnoreRule {
            base: base.to_vec(),
            pattern: rest.to_vec(),
            negated,
            dir_only,
            anchored,
        });
    }
}

/// 行尾未转义的空格在 git 里被剥掉（`foo\ ` 例外：那是字面量空格）。
fn strip_trailing_spaces(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && line[end - 1] == b' ' {
        let mut backslashes = 0;
        let mut i = end - 1;
        while i > 0 && line[i - 1] == b'\\' {
            backslashes += 1;
            i -= 1;
        }
        if backslashes % 2 == 1 {
            break;
        }
        end -= 1;
    }
    &line[..end]
}

/// 规则是否命中 `path`。`base` 不是 `path` 的祖先目录时直接不命中。
fn rule_matches(rule: &IgnoreRule, path: &[u8], is_dir: bool) -> bool {
    let rel = match relative_to(&rule.base, path) {
        Some(rel) => rel,
        None => return false,
    };
    if rel.is_empty() {
        return false;
    }

    if rule.anchored {
        // 逐 component 边界的**前缀**匹配：目录规则因此覆盖整棵子树。
        let mut prefix: Vec<u8> = Vec::new();
        let mut idx = 0usize;
        loop {
            let end = rel[idx..]
                .iter()
                .position(|b| *b == b'/')
                .map(|offset| idx + offset)
                .unwrap_or(rel.len());
            if idx > 0 {
                prefix.push(b'/');
            }
            prefix.extend_from_slice(&rel[idx..end]);
            let last = end == rel.len();
            if !(rule.dir_only && last && !is_dir) && wildmatch(&rule.pattern, &prefix) {
                return true;
            }
            if last {
                return false;
            }
            idx = end + 1;
        }
    } else {
        // 未锚定：对每个 component（含目录层）单独匹配。
        let components: Vec<&[u8]> = rel.split(|b| *b == b'/').collect();
        for (i, component) in components.iter().enumerate() {
            let last = i + 1 == components.len();
            if rule.dir_only && last && !is_dir {
                continue;
            }
            if wildmatch(&rule.pattern, component) {
                return true;
            }
        }
        false
    }
}

/// `path` 相对 `base` 的剩余部分；`base` 为空表示根。
fn relative_to<'p>(base: &[u8], path: &'p [u8]) -> Option<&'p [u8]> {
    if base.is_empty() {
        return Some(path);
    }
    if path.len() > base.len() && path.starts_with(base) && path[base.len()] == b'/' {
        Some(&path[base.len() + 1..])
    } else {
        None
    }
}

/// 通配匹配（gitignore 子集）：`*` 不跨 `/`，`?` 不匹配 `/`，`[...]` 为字符类，
/// `\` 转义下一个字节。`**` 与 `*` 等价（见模块文档）。
fn wildmatch(pattern: &[u8], text: &[u8]) -> bool {
    let mut p = 0usize;
    let mut t = 0usize;
    // 最近一个 `*` 的回溯点：(星号之后的模式位置, 星号开始消费的 text 位置)
    let mut star: Option<(usize, usize)> = None;

    while t < text.len() {
        if p < pattern.len() && pattern[p] == b'*' {
            let mut q = p;
            while q < pattern.len() && pattern[q] == b'*' {
                q += 1;
            }
            star = Some((q, t));
            p = q;
            continue;
        }
        if p < pattern.len() {
            if let Some(used) = match_unit(&pattern[p..], text[t]) {
                p += used;
                t += 1;
                continue;
            }
        }
        match star {
            Some((sp, st)) => {
                // `*` 不跨 `/`：星号无法吞掉一个 `/`，此处已无别的回溯点。
                if text[st] == b'/' {
                    return false;
                }
                star = Some((sp, st + 1));
                p = sp;
                t = st + 1;
            }
            None => return false,
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// 匹配单个模式单元，返回消耗的模式字节数。
fn match_unit(pattern: &[u8], ch: u8) -> Option<usize> {
    match pattern.first().copied()? {
        b'?' if ch != b'/' => Some(1),
        b'\\' if pattern.len() > 1 => (pattern[1] == ch).then_some(2),
        b'[' => match match_class(pattern, ch) {
            Some((true, used)) => Some(used),
            Some((false, _)) => None,
            // 截断/损坏的 `[...]`（没有闭合 `]`）按字面量 `[` 处理，绝不 panic。
            None => (ch == b'[').then_some(1),
        },
        c if c == ch => Some(1),
        _ => None,
    }
}

/// 从 `pat[0] == b'['` 起匹配方括号字符类。
/// 返回 `Some((是否命中, 消耗字节数))`；没有闭合 `]` 时返回 `None`。
fn match_class(pat: &[u8], ch: u8) -> Option<(bool, usize)> {
    let mut i = 1usize;
    let mut negated = false;
    if i < pat.len() && (pat[i] == b'!' || pat[i] == b'^') {
        negated = true;
        i += 1;
    }

    let mut matched = false;
    let mut first = true;
    let mut closed = false;
    while i < pat.len() {
        let c = pat[i];
        if c == b']' && !first {
            i += 1;
            closed = true;
            break;
        }
        first = false;

        let (lo, next) = if c == b'\\' && i + 1 < pat.len() {
            (pat[i + 1], i + 2)
        } else {
            (c, i + 1)
        };
        if next + 1 < pat.len() && pat[next] == b'-' && pat[next + 1] != b']' {
            let (hi, after) = if pat[next + 1] == b'\\' && next + 2 < pat.len() {
                (pat[next + 2], next + 3)
            } else {
                (pat[next + 1], next + 2)
            };
            if ch != b'/' && (lo..=hi).contains(&ch) {
                matched = true;
            }
            i = after;
        } else {
            if ch != b'/' && ch == lo {
                matched = true;
            }
            i = next;
        }
    }

    if !closed {
        return None;
    }
    Some((matched != negated, i))
}

pub(crate) fn os_bytes(name: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        name.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        name.to_string_lossy().into_owned().into_bytes()
    }
}

/// 拼出仓库相对路径：`base + "/" + name`（`base` 为空时即 `name`）。
pub(crate) fn join_rel(base: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(base.len() + name.len() + 1);
    if !base.is_empty() {
        out.extend_from_slice(base);
        out.push(b'/');
    }
    out.extend_from_slice(name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, negated: bool, dir_only: bool, anchored: bool) -> IgnoreRule {
        IgnoreRule {
            base: Vec::new(),
            pattern: pattern.as_bytes().to_vec(),
            negated,
            dir_only,
            anchored,
        }
    }

    fn is_ignored(rules: Vec<IgnoreRule>, path: &str, is_dir: bool) -> bool {
        Ignore::from_rules(rules).is_ignored(path.as_bytes(), is_dir)
    }

    #[test]
    fn dir_only_rule_covers_subtree_for_non_dir_and_dir() {
        let rules = vec![rule("build", false, true, false)];
        assert!(is_ignored(rules.clone(), "build/x", false));
        assert!(is_ignored(rules.clone(), "a/build/x/y.txt", false));
        assert!(is_ignored(rules.clone(), "build", true));
        // 同名的普通文件不被 `build/` 命中。
        assert!(!is_ignored(rules, "build", false));
    }

    #[test]
    fn last_matching_rule_wins_so_negation_rescues() {
        let rules = vec![
            rule("*.txt", false, false, false),
            rule("important.txt", true, false, false),
        ];
        assert!(is_ignored(rules.clone(), "notes.txt", false));
        assert!(!is_ignored(rules.clone(), "important.txt", false));
        // 反选也作用于子目录里的同名文件（未锚定模式）。
        assert!(!is_ignored(rules, "sub/important.txt", false));
    }

    #[test]
    fn anchored_rule_with_base_is_scoped_to_that_directory() {
        let rules = vec![IgnoreRule {
            base: b"sub".to_vec(),
            // 解析器会把前导 `/` 剥掉并置 `anchored`；这里直接构造等价规则。
            pattern: b"keep.txt".to_vec(),
            negated: false,
            dir_only: false,
            anchored: true,
        }];
        assert!(is_ignored(rules.clone(), "sub/keep.txt", false));
        assert!(!is_ignored(rules.clone(), "sub/deep/keep.txt", false));
        assert!(!is_ignored(rules, "keep.txt", false));
    }

    #[test]
    fn anchored_pattern_only_matches_at_base() {
        let rules = vec![rule("a.txt", false, false, true)];
        assert!(is_ignored(rules.clone(), "a.txt", false));
        assert!(!is_ignored(rules, "sub/a.txt", false));

        let nested = vec![rule("doc/frotz", false, false, true)];
        assert!(is_ignored(nested.clone(), "doc/frotz", false));
        assert!(!is_ignored(nested, "a/doc/frotz", false));
    }

    #[test]
    fn wildcards_and_classes() {
        let rules = vec![rule("*.log", false, false, false)];
        assert!(is_ignored(rules.clone(), "x.log", false));
        assert!(is_ignored(rules.clone(), "a/b/x.log", false));
        assert!(!is_ignored(rules, "x.log.bak", false));

        let q = vec![rule("fo?", false, false, false)];
        assert!(is_ignored(q.clone(), "foo", false));
        assert!(!is_ignored(q.clone(), "fo/bar", false));
        assert!(!is_ignored(q, "fo", false));

        let class = vec![rule("[abc]x", false, false, false)];
        assert!(is_ignored(class.clone(), "ax", false));
        assert!(!is_ignored(class.clone(), "zx", false));
        let negated_class = vec![rule("[!abc]x", false, false, false)];
        assert!(is_ignored(negated_class.clone(), "zx", false));
        assert!(!is_ignored(negated_class, "ax", false));
    }

    #[test]
    fn malformed_patterns_do_not_panic() {
        for pattern in ["[", "[abc", "[a-", "a[b", "\\", "*[", "!"] {
            let rules = vec![rule(pattern, false, false, false)];
            let _ = is_ignored(rules, "whatever.txt", false);
        }
    }

    #[test]
    fn parses_comments_blanks_and_trailing_escaped_space() {
        let text = b"# comment\n\n  \n*.tmp\nfoo\\ \n";
        let mut rules = Vec::new();
        parse_gitignore(text, &[], &mut rules);
        assert_eq!(
            rules.len(),
            2,
            "comment/blank lines must be skipped: {rules:?}"
        );
        assert_eq!(rules[0].pattern, b"*.tmp");
        assert_eq!(
            rules[1].pattern, b"foo\\ ",
            "escaped trailing space is kept"
        );
        assert!(Ignore::from_rules(rules).is_ignored(b"foo ", false));
    }

    #[test]
    fn root_dir_only_slash_is_ignored_gracefully() {
        let mut rules = Vec::new();
        parse_gitignore(b"/\n!\n!\n", &[], &mut rules);
        assert!(Ignore::from_rules(rules).rules().is_empty());
    }

    #[test]
    fn loads_nested_gitignore_with_bases() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repo::init(dir.path(), "main").expect("init");
        std::fs::write(dir.path().join(".gitignore"), b"*.txt\n").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/.gitignore"), b"!keep.txt\n").unwrap();

        let ignore = Ignore::load(&repo).expect("load");
        assert!(ignore
            .rules()
            .iter()
            .all(|r| r.base.is_empty() || r.base == b"sub"));
        assert!(ignore.is_ignored(b"a.txt", false));
        assert!(
            !ignore.is_ignored(b"sub/keep.txt", false),
            "sub rule rescues"
        );
        assert!(ignore.is_ignored(b"sub/other.txt", false));

        let sub_rules: Vec<&IgnoreRule> =
            ignore.rules().iter().filter(|r| r.base == b"sub").collect();
        assert_eq!(sub_rules.len(), 1);
        assert!(sub_rules[0].negated);
    }

    #[test]
    fn ignored_directory_is_not_descended_for_rule_loading() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repo::init(dir.path(), "main").expect("init");
        std::fs::write(dir.path().join(".gitignore"), b"build/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("build")).unwrap();
        std::fs::write(dir.path().join("build/.gitignore"), b"!keep.txt\n").unwrap();

        let ignore = Ignore::load(&repo).expect("load");
        // 与 git 一致：被忽略目录内部的规则不会生效。
        assert!(ignore.is_ignored(b"build/keep.txt", false));
    }

    #[test]
    fn non_utf8_gitignore_does_not_fail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = Repo::init(dir.path(), "main").expect("init");
        std::fs::write(dir.path().join(".gitignore"), b"\xff\xfe\n*.bin\n").unwrap();
        let ignore = Ignore::load(&repo).expect("load");
        assert!(ignore.is_ignored(b"x.bin", false));
    }
}
