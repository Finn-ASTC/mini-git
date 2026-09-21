//! `mg clone` —— **T13（codex，file://）/ T14（opencode，http://）**。
//!
//! 验收：`mg clone file:///tmp/src dst` 后，dst 里 `git fsck` 通过、
//! `git log --oneline` 与源仓库一致、`git status` 干净。

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::transport::local;

pub fn run(url: &str, dir: Option<&Path>) -> Result<()> {
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => default_dir(url)?,
    };
    // 真实 git 把 clone 的进度全打到 stderr（stdout 为空）；照抄，空远端 warning 也在 stderr。
    eprintln!("Cloning into '{}'...", dir.display());
    local::clone_into(url, &dir)?;
    Ok(())
}

/// 真实 git 的默认目标目录：URL 的最后一段，去掉结尾的 `/` 与 `.git`。
fn default_dir(url: &str) -> Result<PathBuf> {
    // 先去掉 `<scheme>://`，再从路径最后一段取名（真实 git 也忽略 URL 的 scheme）。
    let path = match url.find("://") {
        Some(idx) => &url[idx + 3..],
        None => url,
    };
    let trimmed = path.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or("");
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() || name == "." || name == ".." {
        return Err(Error::Other(format!(
            "cannot derive a directory name from '{url}'; give one explicitly"
        )));
    }
    Ok(PathBuf::from(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_the_directory_name_like_git() {
        assert_eq!(
            default_dir("file:///tmp/src").unwrap(),
            PathBuf::from("src")
        );
        assert_eq!(
            default_dir("file:///tmp/src/").unwrap(),
            PathBuf::from("src")
        );
        assert_eq!(
            default_dir("http://host/x/y.git").unwrap(),
            PathBuf::from("y")
        );
        assert!(default_dir("file:///").is_err());
    }
}
