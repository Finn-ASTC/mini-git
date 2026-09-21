//! `mg init` —— W0 已实现（验收：`git -C <dir> status` 能正常工作）。

use std::path::Path;

use crate::error::Result;
use crate::repo::Repo;

pub fn run(path: Option<&Path>, initial_branch: &str, quiet: bool) -> Result<()> {
    let target = path.unwrap_or_else(|| Path::new("."));
    let repo = Repo::init(target, initial_branch)?;
    if !quiet {
        println!(
            "Initialized empty mini-git repository in {}",
            repo.git_dir().display()
        );
    }
    Ok(())
}
