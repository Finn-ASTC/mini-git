//! `.git/index`（DIRC v2）：工作区 ↔ 提交之间的暂存区。
//!
//! **CONTROLLER-OWNED（`pub` 声明区冻结）**；`dirc.rs` 属于 **T3**（opencode）。
//!
//! 这是全项目最容易翻车的二进制格式：entry 需要 8 字节对齐填充、
//! flags 里塞了 stage 和 name 长度、末尾有整体 SHA-1 trailer、
//! 还有必须原样保留的未知扩展（读→写不能丢数据）。
//! 详见 `ORCHESTRATION.md` §2 与 `dirc.rs` 头部格式表。
//!
//! 验收：读真实 git 的 index → 重写 → `git ls-files --stage` 输出不变。

pub mod dirc;

use std::fs::Metadata;

use crate::error::Result;
use crate::object::FileMode;
use crate::oid::Oid;
use crate::repo::Repo;

pub use dirc::{read_index, write_index};

/// TREE 扩展签名（缓存 index 对应的 tree oid）。
pub const TREE_EXTENSION: [u8; 4] = *b"TREE";

/// 已知扩展签名白名单；不在其中的一律原样透传。
pub const KNOWN_EXTENSIONS: [&[u8; 4]; 2] = [b"TREE", b"REUC"];

/// index entry 的 stat 缓存（用于跳过重新哈希）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatData {
    pub ctime_s: u32,
    pub ctime_ns: u32,
    pub mtime_s: u32,
    pub mtime_ns: u32,
    pub dev: u32,
    pub ino: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u32,
}

impl StatData {
    pub fn from_metadata(meta: &Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            StatData {
                ctime_s: meta.ctime() as u32,
                ctime_ns: meta.ctime_nsec() as u32,
                mtime_s: meta.mtime() as u32,
                mtime_ns: meta.mtime_nsec() as u32,
                dev: meta.dev() as u32,
                ino: meta.ino() as u32,
                uid: meta.uid(),
                gid: meta.gid(),
                size: meta.len() as u32,
            }
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            StatData::default()
        }
    }
}

/// 一个 index 条目。同一路径在冲突时会有 stage 1/2/3 多条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    /// 仓库相对路径，`/` 分隔，UTF-8 字节。
    pub path: Vec<u8>,
    pub oid: Oid,
    pub mode: FileMode,
    pub stat: StatData,
    /// 0 = 正常；1/2/3 = 冲突的 base/ours/theirs。
    pub stage: u8,
    pub assume_valid: bool,
    pub extended: bool,
    /// 原始 flags，用于无损往返（含 skip-worktree / intent-to-add 等位）。
    pub flags_raw: u16,
}

impl IndexEntry {
    pub fn new(path: impl Into<Vec<u8>>, oid: Oid, mode: FileMode) -> Self {
        IndexEntry {
            path: path.into(),
            oid,
            mode,
            stat: StatData::default(),
            stage: 0,
            assume_valid: false,
            extended: false,
            flags_raw: 0,
        }
    }

    pub fn is_stage0(&self) -> bool {
        self.stage == 0
    }

    /// racy-git 判断：stat 完全命中时才可以信任缓存。
    pub fn stat_matches(&self, meta: &Metadata) -> bool {
        let live = StatData::from_metadata(meta);
        self.stat.mtime_s == live.mtime_s
            && self.stat.mtime_ns == live.mtime_ns
            && self.stat.size == live.size
    }
}

/// 未识别的 index 扩展，原样保留以保证往返无损。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    pub signature: [u8; 4],
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Index {
    pub version: u32,
    /// 已按 (path, stage) 排序。
    pub entries: Vec<IndexEntry>,
    /// TREE 扩展缓存的 tree oid。
    pub tree_oid: Option<Oid>,
    /// 其他未知扩展，按原顺序保存。
    pub extensions: Vec<Extension>,
}

impl Index {
    /// 读 `.git/index`；文件不存在时返回空 index（不是错误）。
    pub fn read(repo: &Repo) -> Result<Index> {
        dirc::read_index(repo)
    }

    /// 原子写回 `.git/index`（临时文件 + rename）。
    pub fn write(&self, repo: &Repo) -> Result<()> {
        dirc::write_index(repo, self)
    }

    pub fn lookup(&self, path: &[u8]) -> Option<&IndexEntry> {
        self.entries
            .iter()
            .find(|entry| entry.path == path && entry.stage == 0)
    }

    pub fn lookup_stage(&self, path: &[u8], stage: u8) -> Option<&IndexEntry> {
        self.entries
            .iter()
            .find(|entry| entry.path == path && entry.stage == stage)
    }

    pub fn upsert(&mut self, entry: IndexEntry) {
        match self
            .entries
            .iter_mut()
            .find(|existing| existing.path == entry.path && existing.stage == entry.stage)
        {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
        self.sort();
    }

    /// 删除某路径的 stage 0 条目，返回被删掉的条目。
    pub fn remove(&mut self, path: &[u8]) -> Option<IndexEntry> {
        let idx = self
            .entries
            .iter()
            .position(|entry| entry.path == path && entry.stage == 0)?;
        Some(self.entries.remove(idx))
    }

    /// 删除某路径的所有 stage（冲突清理用）。
    pub fn remove_all_stages(&mut self, path: &[u8]) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.path != path);
        self.entries.len() != before
    }

    pub fn has_conflicts(&self) -> bool {
        self.entries.iter().any(|entry| entry.stage != 0)
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &IndexEntry> {
        self.entries.iter().filter(|entry| entry.stage != 0)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn sort(&mut self) {
        self.entries
            .sort_by(|a, b| a.path.cmp(&b.path).then(a.stage.cmp(&b.stage)));
    }
}
