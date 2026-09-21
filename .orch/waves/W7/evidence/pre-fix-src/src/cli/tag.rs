//! `mg tag` —— **T12（hermes）**。
//!
//! * 轻量 tag：把解析出的 oid 写进 `refs/tags/<name>`；
//! * `-a` / `-m`：先写 tag 对象（`object` / `type` / `tag` / `tagger` 行 + 消息），
//!   再把 ref 指向它。`-m` 单独出现时**隐含** `-a`（与 git 一致）；
//! * `-l`：列出 tag 名（按 refname 字节序，与 `git tag -l` 相同）；给了 name 时按
//!   简单通配（`*` / `?`）过滤（`git tag -l <pattern>`）；
//! * 创建标签时用 CAS（`expected = None`，必须不存在）→ 已存在时报错，与 git 相同。
//!
//! 消息清理复用 `cli::commit::cleanup_message`（实测 git 的 tag 消息同样经过
//! `strbuf_stripspace`：行尾空白与首尾空行被去掉，末尾补一个换行）。
//!
//! 已知限制：
//! * 删除 tag（`-d`）不在本轮签名内；
//! * `-l -n[<n>]` / `--format` 不在签名内；
//! * 不支持 GPG 签名 tag、不支持 `-f` 覆盖同名 tag；
//! * 只做基础 refname 合法性检查（git 的 `check_refname_format` 子集）。

use crate::error::{Error, Result};
use crate::object::{Kind, Object, Tag};
use crate::odb::Odb;
use crate::oid::Oid;
use crate::refs::{RefStore, TAGS_PREFIX};

use super::commit::{cleanup_message, identity, local_timezone_offset, now_unix};

pub fn run(
    annotate: bool,
    message: Option<&str>,
    list: bool,
    name: Option<&str>,
    rev: Option<&str>,
) -> Result<()> {
    let repo = crate::cli::open_repo()?;
    let store = RefStore::new(&repo);

    if list {
        return list_tags(&store, name);
    }

    let Some(name) = name else {
        // git 的 `git tag`（无参数）等价 `git tag -l`；`-a`/`-m` 缺名字则是用法错误。
        if annotate || message.is_some() {
            return Err(Error::Other("tag name required".to_string()));
        }
        return list_tags(&store, None);
    };

    let refname = format!("{TAGS_PREFIX}{name}");
    check_refname(&refname)?;
    if store.exists(&refname) {
        return Err(Error::Other(format!("tag '{name}' already exists")));
    }

    let target = store.resolve(rev.unwrap_or("HEAD"))?;
    let odb = Odb::new(&repo);

    // `-m` 隐含 `-a`（git 的 `git tag -m msg <name>` 会创建 annotated tag）。
    let oid = if annotate || message.is_some() {
        let Some(text) = message else {
            return Err(Error::Other(
                "no tag message given and mini-git does not open an editor (use -m)".to_string(),
            ));
        };
        let when = now_unix();
        let tz = local_timezone_offset();
        let tagger = identity(&repo).to_signature(when, &tz);
        let tag = Tag {
            object: target,
            kind: object_kind(&odb, target)?,
            name: name.as_bytes().to_vec(),
            tagger: Some(tagger),
            message: cleanup_message(text),
            extra_headers: Vec::new(),
        };
        odb.write(Kind::Tag, &tag.encode_payload())?
    } else {
        target
    };

    // 创建语义：ref 必须还不存在（CAS），已存在时 store 会报 RefConflict；
    // 上面已经先查过一次，这里保留 CAS 以防并发写者插队。
    store.update(&refname, oid, None)?;
    Ok(())
}

fn list_tags(store: &RefStore<'_>, pattern: Option<&str>) -> Result<()> {
    let mut names: Vec<String> = store
        .list()?
        .into_iter()
        .filter_map(|(name, _)| {
            name.strip_prefix(TAGS_PREFIX)
                .map(|short| short.to_string())
        })
        .collect();
    names.sort();
    for name in names {
        if let Some(pattern) = pattern {
            if !glob_matches(pattern.as_bytes(), name.as_bytes()) {
                continue;
            }
        }
        println!("{name}");
    }
    Ok(())
}

fn object_kind(odb: &Odb<'_>, oid: Oid) -> Result<Kind> {
    match odb.read_object(oid) {
        Ok(Object::Commit(_)) => Ok(Kind::Commit),
        Ok(Object::Tree(_)) => Ok(Kind::Tree),
        Ok(Object::Blob(_)) => Ok(Kind::Blob),
        Ok(Object::Tag(_)) => Ok(Kind::Tag),
        Err(err) => Err(err),
    }
}

/// `git tag -l <pattern>` 用的简化 glob：`*`（可跨 `/`）与 `?`。
fn glob_matches(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|skip| glob_matches(rest, &text[skip..])),
        Some((b'?', rest)) => match text.split_first() {
            Some((_, tail)) => glob_matches(rest, tail),
            None => false,
        },
        Some((byte, rest)) => match text.split_first() {
            Some((head, tail)) if head == byte => glob_matches(rest, tail),
            _ => false,
        },
    }
}

/// refname 合法性（`check_refname_format` 的常用子集）。
fn check_refname(refname: &str) -> Result<()> {
    let bad = |reason: &str| {
        Err(Error::Other(format!(
            "invalid tag name {refname:?}: {reason}"
        )))
    };
    let short = refname.strip_prefix(TAGS_PREFIX).unwrap_or(refname);
    if short.is_empty() {
        return bad("empty name");
    }
    if short.starts_with('/') || short.ends_with('/') || short.contains("//") {
        return bad("empty path component");
    }
    if short.ends_with('.') || short.ends_with(".lock") {
        return bad("name must not end with '.' or '.lock'");
    }
    if short.contains("..") || short.contains("@{") {
        return bad("forbidden sequence");
    }
    for byte in short.bytes() {
        if byte < 0x20 || byte == 0x7f || b" ~^:?*[\\".contains(&byte) {
            return bad("forbidden character");
        }
        if byte == b'>' || byte == b'<' || byte == b'"' || byte == b'|' || byte == b'&' {
            return bad("forbidden character");
        }
    }
    for component in short.split('/') {
        if component.starts_with('.') {
            return bad("path component must not start with '.'");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn glob_matches_git_tag_l_patterns() {
        assert!(glob_matches(b"v*", b"v1.0"));
        assert!(glob_matches(b"v*", b"v"));
        assert!(!glob_matches(b"v*", b"release"));
        assert!(glob_matches(b"v?", b"v1"));
        assert!(!glob_matches(b"v?", b"v12"));
        assert!(glob_matches(b"*", b""));
        assert!(glob_matches(b"release/*", b"release/1.0"));
        assert!(!glob_matches(b"release/*", b"release"));
    }

    #[test]
    fn refname_validation_rejects_git_forbidden_names() {
        for bad in [
            "",
            "/abs",
            "trailing/",
            "double//slash",
            "ends.lock",
            "ends.",
            "has~tilde",
            "has^caret",
            "has:colon",
            "has space",
            "has..dots",
            "at@{brace",
            ".hidden",
            "ref/..",
        ] {
            assert!(
                check_refname(&format!("refs/tags/{bad}")).is_err(),
                "{bad:?} must be rejected"
            );
        }
        for good in ["v1.0", "v1.0-rc1", "release/2024-01", "中文标签"] {
            assert!(
                check_refname(&format!("refs/tags/{good}")).is_ok(),
                "{good:?} must be accepted"
            );
        }
    }

    /// 消息清理与 git 的 annotated tag 出一致字节（真值现场取自 `git tag -a -m`）。
    #[test]
    fn tag_message_cleanup_matches_git() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(git(dir, &["init", "-q", "-b", "main"]).status.success());
        std::fs::write(dir.join("f"), b"f\n").unwrap();
        assert!(git(dir, &["add", "-A"]).status.success());
        assert!(git(dir, &["commit", "-q", "-m", "init"]).status.success());

        for raw in ["tag message", "a\n\n\n", "x   \n\ny"] {
            assert!(git(dir, &["tag", "-a", "probe", "-m", raw, "-f"])
                .status
                .success());
            let payload = git(dir, &["cat-file", "tag", "probe"]);
            let text = String::from_utf8_lossy(&payload.stdout).into_owned();
            let want = text
                .split_once("\n\n")
                .map(|(_, body)| body.to_string())
                .expect("tag payload has a blank line");
            let got = String::from_utf8(cleanup_message(raw)).unwrap();
            assert_eq!(got, want, "tagger message mismatch for {raw:?}");
            assert!(git(dir, &["tag", "-d", "probe"]).status.success());
        }
    }
}
