//! `mg push` —— **T13（codex）/ T14（opencode）**。
//!
//! 必须拒绝 non-fast-forward（除非 `--force`），并解析 `report-status` 的错误原因。
//! 验收：`git clone` 出来的仓库能看到 mg push 的提交；non-FF 时退出码非 0 且远端未被改动。

use crate::cli::fetch::remote_and_url;
use crate::cli::open_repo;
use crate::error::{Error, Result};
use crate::refs::{Head, RefStore, HEADS_PREFIX};
use crate::transport::local;

pub fn run(
    remote: Option<&str>,
    refspec: Option<&str>,
    set_upstream: bool,
    force: bool,
) -> Result<()> {
    let repo = open_repo()?;
    let (name, url) = remote_and_url(&repo, remote)?;
    let refspecs: Vec<String> = match refspec {
        Some(text) => vec![text.to_string()],
        None => Vec::new(),
    };
    local::push(&repo, &url, &refspecs, force)?;

    if set_upstream {
        let branch = match RefStore::new(&repo).read_head()? {
            Head::Attached(target) => target
                .strip_prefix(HEADS_PREFIX)
                .unwrap_or(target.as_str())
                .to_string(),
            Head::Detached(_) => {
                return Err(Error::Other(
                    "cannot set upstream: HEAD is detached".to_string(),
                ))
            }
        };
        let merge_ref = match refspec.and_then(|text| text.split_once(':')).map(|(_, d)| d) {
            Some(dest) if dest.starts_with("refs/") => dest.to_string(),
            Some(dest) => format!("{HEADS_PREFIX}{dest}"),
            None => format!("{HEADS_PREFIX}{branch}"),
        };
        local::set_branch_upstream(&repo, &branch, &name, &merge_ref)?;
        println!(
            "branch '{branch}' set up to track '{name}' with {merge_ref}."
        );
    }
    Ok(())
}
