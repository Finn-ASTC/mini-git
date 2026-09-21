//! `mg gc` —— **T15（omp）**。
//!
//! v1 范围：把 loose object 打包成 `.pack`/`.idx`、写 `packed-refs`、删掉已被打包的 loose 对象。
//! 验收：`mg gc` 后 `git fsck` 通过、`git log --all` 正常、对象数下降，
//! 且 `git gc` 能把 mg 生成的 pack 正常读取。

use crate::error::{todo, Result};

pub fn run() -> Result<()> {
    todo("cli::gc (T15)")
}
