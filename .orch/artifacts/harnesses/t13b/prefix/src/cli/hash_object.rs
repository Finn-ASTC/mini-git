//! `mg hash-object` —— 不带 `-w` 的部分 W0 已实现（纯计算）。
//! 带 `-w` 依赖 `odb::loose::write_loose`，属 **T5（hermes）**。
//!
//! 验收：`printf 'hello\n' | mg hash-object --stdin` == `git hash-object --stdin`。

use std::io::Read;
use std::path::Path;

use crate::error::{Error, Result};
use crate::object::{self, Kind};
use crate::odb::Odb;
use crate::repo::Repo;

pub fn run(file: Option<&Path>, stdin: bool, write: bool, kind: &str) -> Result<()> {
    let kind = Kind::from_bytes(kind.as_bytes())?;

    let payload = if stdin {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)?;
        buf
    } else {
        let path =
            file.ok_or_else(|| Error::Other("hash-object needs a file or --stdin".to_string()))?;
        std::fs::read(path)?
    };

    let oid = if write {
        let repo = Repo::discover(std::path::Path::new("."))?;
        Odb::new(&repo).write(kind, &payload)?
    } else {
        object::hash(kind, &payload)
    };
    println!("{oid}");
    Ok(())
}
