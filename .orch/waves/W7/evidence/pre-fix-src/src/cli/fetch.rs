//! `mg fetch` —— **T13（codex）/ T14（opencode）**。
//!
//! 更新的引用写到 `refs/remotes/<remote>/...`，并把 `FETCH_HEAD` 写好。
//! 验收：fetch 后 `git show-ref` 的远端引用存在且 oid 正确。

use std::path::Path;

use crate::cli::open_repo;
use crate::error::{Error, Result};
use crate::repo::Repo;
use crate::transport::local;

pub fn run(remote: Option<&str>, refspec: Option<&str>) -> Result<()> {
    let repo = open_repo()?;
    let (name, url) = remote_and_url(&repo, remote)?;
    let refspecs: Vec<String> = match refspec {
        Some(text) => vec![text.to_string()],
        None => vec![format!("+refs/heads/*:refs/remotes/{name}/*")],
    };
    let outcome = local::fetch(&repo, &url, &refspecs)?;
    // 真实 git 只在**确实更新了引用**时才打印 `From <url>`：空远端、通配 refspec 命中 0 个引用、
    // 已是最新时 stdout/stderr 全空（实测 git 2.55.0）；这里照抄。
    if !outcome.refs.is_empty() {
        println!("From {url}");
    }
    for (dest, oid) in &outcome.refs {
        println!(" * [updated] {dest} -> {}", &oid.to_hex()[..7]);
    }
    Ok(())
}

/// 远端参数 → `(远端名, url)`。
///
/// * 已配置的远端（`remote.<name>.url`）：直接用。
/// * 没配置但看起来是 URL / 路径：把它当 URL，引用记在 `origin` 名下
///   （真实 git 的 `git fetch <url>` 只写 `FETCH_HEAD`，这里多做了一步，
///   因为 mg 的 pull/后续 fetch 都依赖 `refs/remotes/<name>/*`）。
pub(crate) fn remote_and_url(repo: &Repo, remote: Option<&str>) -> Result<(String, String)> {
    let name = remote
        .map(str::to_string)
        .unwrap_or_else(|| local::DEFAULT_REMOTE.to_string());
    if let Some(url) = repo.config().get(&format!("remote.{name}.url")) {
        return Ok((name, url.to_string()));
    }
    let looks_like_url = name.contains("://")
        || name.starts_with('/')
        || name.starts_with("./")
        || name.starts_with("../")
        || name.starts_with("file:")
        || Path::new(&name).exists();
    if remote.is_some() && looks_like_url {
        return Ok((local::DEFAULT_REMOTE.to_string(), name));
    }
    Err(Error::Other(format!(
        "'{name}' does not appear to be a git repository, and it is not a configured remote"
    )))
}
