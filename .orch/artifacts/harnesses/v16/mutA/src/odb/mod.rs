//! 对象数据库（ODB）：loose objects + packfiles 的统一读接口。
//!
//! **CONTROLLER-OWNED（`pub` 声明区冻结）**；`loose.rs` 属 **T5（hermes）**，
//! `pack/` 属 **T9（codex）**。
//!
//! 读取顺序：先 loose，再 pack。写入一律先写 loose（pack 由 `mg gc` 生成）。

pub mod loose;
pub mod pack;

use crate::error::Result;
use crate::object::{self, Kind, Object};
use crate::oid::Oid;
use crate::repo::Repo;

pub use pack::PackSet;

pub struct Odb<'a> {
    repo: &'a Repo,
}

impl<'a> Odb<'a> {
    pub fn new(repo: &'a Repo) -> Self {
        Odb { repo }
    }

    pub fn repo(&self) -> &'a Repo {
        self.repo
    }

    /// 读对象：loose 优先；未命中则回退到 pack。
    pub fn read(&self, oid: Oid) -> Result<(Kind, Vec<u8>)> {
        if let Some(found) = loose::read_loose(self.repo, oid)? {
            return Ok(found);
        }
        match pack::PackSet::open(self.repo)?.read(oid)? {
            Some(found) => Ok(found),
            None => Err(crate::error::Error::ObjectNotFound(oid)),
        }
    }

    pub fn read_object(&self, oid: Oid) -> Result<Object> {
        let (kind, payload) = self.read(oid)?;
        Object::decode(kind, &payload)
    }

    /// 写对象（幂等）：已存在则直接返回其 id。
    pub fn write(&self, kind: Kind, payload: &[u8]) -> Result<Oid> {
        loose::write_loose(self.repo, kind, payload)
    }

    pub fn exists(&self, oid: Oid) -> bool {
        loose::loose_path(self.repo, oid).is_file()
    }

    /// 仅 loose 对象 id。
    pub fn iter_loose(&self) -> Result<Vec<Oid>> {
        loose::iter_loose(self.repo)
    }

    /// loose ∪ packed 的全部对象 id（`mg fsck` / `mg gc` 用）。
    pub fn iter_all(&self) -> Result<Vec<Oid>> {
        let mut oids = self.iter_loose()?;
        oids.extend(pack::PackSet::open(self.repo)?.iter_oids()?);
        oids.sort();
        oids.dedup();
        Ok(oids)
    }

    /// 校验某对象的 id 与其内容一致（`mg fsck` 用）。
    pub fn verify(&self, oid: Oid) -> Result<bool> {
        let raw = loose::read_loose_raw(self.repo, oid)?;
        let Some(raw) = raw else {
            return Ok(false);
        };
        let (kind, payload) = object::decode(&raw)?;
        Ok(Oid::hash_object(kind.as_str(), &payload) == oid)
    }
}
