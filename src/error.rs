//! 全项目的错误类型。CONTROLLER-OWNED。

use std::path::PathBuf;

use crate::oid::Oid;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not implemented yet: {0}")]
    NotImplemented(&'static str),

    #[error("not a git repository (or any parent up to filesystem boundary): {0}")]
    NotARepo(PathBuf),

    #[error("unsupported repository format version {0} (mini-git supports 0)")]
    UnsupportedRepoFormat(String),

    #[error("invalid object id: {0}")]
    InvalidOid(String),

    #[error("object not found: {0}")]
    ObjectNotFound(Oid),

    #[error("corrupt {what}: {detail}")]
    Corrupt { what: String, detail: String },

    #[error("reference not found: {0}")]
    RefNotFound(String),

    #[error("reference {name} changed under us (expected {expected}, found {actual})")]
    RefConflict {
        name: String,
        expected: String,
        actual: String,
    },

    #[error("pathspec '{0}' did not match any files")]
    PathspecNotFound(String),

    #[error("refusing to overwrite local changes: {0}")]
    WouldLoseChanges(String),

    #[error("merge conflict in {0}")]
    MergeConflict(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("unsupported: {0}")]
    Unsupported(&'static str),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

// 注：W0 骨架期的 `pub(crate) fn todo<T>()` 桩函数已删除（W4 收口）：全部里程碑实现完之后
// 它成为死代码（clippy `dead_code`），而 `Error::NotImplemented` 仍然保留给「明确不做的功能」
// 使用（例如 `--porcelain=v2`、`https://`、thin pack），不受影响。

impl Error {
    pub fn corrupt(what: impl Into<String>, detail: impl Into<String>) -> Self {
        Error::Corrupt {
            what: what.into(),
            detail: detail.into(),
        }
    }
}
