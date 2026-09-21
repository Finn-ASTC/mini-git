//! 工作区：扫描、ignore 匹配、状态比对、物化（checkout）。
//!
//! **CONTROLLER-OWNED（`pub` 声明区冻结）** —— 属于 **T4（codex）**。
//!
//! `status` 的本质是三方差分：
//!
//! | 比较 | 结果列 |
//! |---|---|
//! | HEAD tree vs index | `X`（A/M/D）|
//! | index vs 工作区 | `Y`（M/D）|
//! | 只有工作区 | `??`（untracked，受 ignore 过滤）|
//! | index 里 stage 1/2/3 | `UU/AA/DU/UD/AU/UA` |
//!
//! 验收：`mg status --porcelain` 与 `git status --porcelain` **逐字节**相同。
//! 陷阱：git 会把「整目录都未跟踪」的情况折叠成一条 `dir/`；空目录不出现；
//! 路径排序是字节序；symlink 与可执行位变化算 `M`；racy-git 命中时要重新哈希内容。
//!
//! **接口澄清（controller，W1 冻结）**：`Worktree::status` 的 `head_tree` 参数是
//! **已展平**的 tree —— `Tree::entries()` 里每一项的 `name` 都是**仓库相对全路径**
//! （如 `a/b/c.txt`），而不是单层名字；`FileMode::Tree` 的条目应被忽略。
//! 展平需要读子树对象（odb），因此由调用方（`cli/status.rs`）负责，
//! `worktree` 层不读 odb。这样 `worktree` 与 `odb` 解耦，可被并行实现与独立测试。

mod status;

pub mod ignore;
pub mod materialize;
pub mod scan;

use crate::error::Result;
use crate::object::{FileMode, Tree};
use crate::oid::Oid;
use crate::repo::Repo;

pub use ignore::Ignore;
pub use materialize::MaterializeOptions;
pub use scan::{scan_worktree, WorktreeFile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    /// 类型变化（普通文件 ↔ symlink ↔ 目录）。git 在 porcelain 里输出 `T`，
    /// 冻结枚举初版漏了这个值，W1 的 V4 用真值对拍发现（controller，C-13）。
    TypeChanged,
}

impl ChangeKind {
    pub const fn code(self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
            ChangeKind::TypeChanged => 'T',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    BothModified,
    BothAdded,
    AddedByUs,
    AddedByThem,
    DeletedByUs,
    DeletedByThem,
}

impl ConflictKind {
    pub const fn porcelain_code(self) -> &'static str {
        match self {
            ConflictKind::BothModified => "UU",
            ConflictKind::BothAdded => "AA",
            ConflictKind::AddedByUs => "AU",
            ConflictKind::AddedByThem => "UA",
            ConflictKind::DeletedByUs => "DU",
            ConflictKind::DeletedByThem => "UD",
        }
    }
}

/// 一行 `git status --porcelain` 记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    pub path: Vec<u8>,
    /// `X` 列。
    pub index: Option<ChangeKind>,
    /// `Y` 列。
    pub worktree: Option<ChangeKind>,
    pub conflict: Option<ConflictKind>,
    pub untracked: bool,
}

impl StatusLine {
    pub fn new(path: impl Into<Vec<u8>>) -> Self {
        StatusLine {
            path: path.into(),
            index: None,
            worktree: None,
            conflict: None,
            untracked: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StatusReport {
    /// 已按 path 字节序排序。
    pub lines: Vec<StatusLine>,
}

impl StatusReport {
    pub fn is_clean(&self) -> bool {
        self.lines.is_empty()
    }
}

pub struct Worktree<'a> {
    repo: &'a Repo,
}

impl<'a> Worktree<'a> {
    pub fn new(repo: &'a Repo) -> Self {
        Worktree { repo }
    }

    pub fn repo(&self) -> &'a Repo {
        self.repo
    }

    /// 工作区全量扫描（含 ignored 标记，供 status 与 add 复用）。
    pub fn scan(&self) -> Result<Vec<WorktreeFile>> {
        scan::scan_worktree(self.repo)
    }

    pub fn ignore(&self) -> Result<Ignore> {
        Ignore::load(self.repo)
    }

    /// 把 tree 物化到工作区（checkout/merge/reset --hard 用），返回写出的路径。
    pub fn materialize_tree(&self, tree: &Tree, opts: &MaterializeOptions) -> Result<Vec<Vec<u8>>> {
        materialize::materialize_tree(self.repo, tree, opts)
    }

    /// 按 mode 写出单个 blob（普通文件 / 可执行 / symlink）。
    pub fn write_blob_to(&self, path: &[u8], oid: Oid, mode: FileMode) -> Result<()> {
        materialize::write_blob_to(self.repo, path, oid, mode)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeContent {
    pub bytes: Vec<u8>,
    pub mode: FileMode,
}
