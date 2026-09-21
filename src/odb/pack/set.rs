//! `PackSet` 的实现。**T9（codex）实现范围**。
//!
//! 为什么单独一个文件：`mod.rs` 是 CONTROLLER-OWNED（冻结公共签名），
//! 实现体必须放在模块自己的文件里，这样 author agent 永远不需要改冻结文件。
//!
//! 诚实性原则（继承 W0 的约定）：`.idx` 解析失败一律 `Corrupt` 冒泡，绝不静默跳过；
//! `read` 命中后必须重算对象 id 与内容一致才返回，否则「pack 坏了」会伪装成
//! 「对象不存在 = ObjectNotFound」。

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::object::Kind;
use crate::oid::Oid;
use crate::repo::Repo;

use super::{PackFile, PackIndex, PackSet};

impl PackSet {
    /// 打开仓库中的全部 pack（`objects/pack/*.idx`，多 pack 是常态：`git gc` 之后
    /// 可能残留旧 pack，`fetch` 又会新建一个）。
    ///
    /// **诚实性原则**：存在 `.idx` 却解析不了 → `Corrupt`（不许当成「没有 pack」，
    /// 那会让 `Odb::read` 把「pack 里有对象」误报成 `ObjectNotFound`，正是本项目最想避免的假绿）。
    pub fn open(repo: &Repo) -> Result<PackSet> {
        let dir = repo.objects_dir().join("pack");
        let mut paths: Vec<PathBuf> = Vec::new();
        if dir.is_dir() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.extension().is_some_and(|ext| ext == "idx") {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        let mut indices = Vec::with_capacity(paths.len());
        for path in paths {
            indices.push(PackIndex::open(&path)?);
        }
        Ok(PackSet { indices })
    }

    /// 在某个 idx 里命中 → 按偏移读出 → **重算对象 id** 校验后返回。
    /// 完全没命中才返回 `Ok(None)`（`Odb::read` 会把它翻译成 `ObjectNotFound`）。
    pub fn read(&self, oid: Oid) -> Result<Option<(Kind, Vec<u8>)>> {
        for idx in &self.indices {
            let Some(offset) = idx.lookup(oid) else {
                continue;
            };
            let pack_path = idx.pack_path().ok_or_else(|| {
                Error::corrupt(
                    idx.label(),
                    "index has no source path: cannot find its .pack",
                )
            })?;
            if !pack_path.is_file() {
                return Err(Error::corrupt(
                    pack_path.display().to_string(),
                    format!(
                        "{} lists {oid} but the matching .pack file is missing",
                        idx.label()
                    ),
                ));
            }
            let pack = PackFile::open(&pack_path)?;
            let (kind, payload) = pack.read_at(offset)?;
            let actual = Oid::hash_object(kind.as_str(), &payload);
            if actual != oid {
                return Err(Error::corrupt(
                    pack_path.display().to_string(),
                    format!(
                        "{} maps {oid} to offset {offset}, whose contents hash to {actual}",
                        idx.label()
                    ),
                ));
            }
            return Ok(Some((kind, payload)));
        }
        Ok(None)
    }

    pub fn contains(&self, oid: Oid) -> bool {
        self.indices.iter().any(|idx| idx.lookup(oid).is_some())
    }

    /// 全部 pack 的 oid 并集（升序、去重），与
    /// `git cat-file --batch-all-objects --batch-check` 的集合一致。
    pub fn iter_oids(&self) -> Result<Vec<Oid>> {
        let mut oids: Vec<Oid> = Vec::new();
        for idx in &self.indices {
            oids.extend_from_slice(idx.iter_oids());
        }
        oids.sort_unstable();
        oids.dedup();
        Ok(oids)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::super::read::gitkit;
    use super::*;
    use crate::odb::Odb;

    fn expected_oids(objects: &[(Oid, String, u64)]) -> Vec<Oid> {
        let mut oids: Vec<Oid> = objects.iter().map(|(oid, _, _)| *oid).collect();
        oids.sort_unstable();
        oids.dedup();
        oids
    }

    /// 对拍：`PackSet` 的集合与每个对象的 `(kind, payload)` 都必须与真 git 一致。
    fn assert_pack_set_matches_git(dir: &Path) {
        let repo = Repo::discover(dir).unwrap();
        let set = PackSet::open(&repo).unwrap();
        assert!(!set.indices.is_empty());

        let objects = gitkit::git_objects(dir);
        let expected = expected_oids(&objects);
        assert_eq!(set.iter_oids().unwrap(), expected);

        for (oid, kind_name, size) in &objects {
            assert!(set.contains(*oid), "{oid} missing from PackSet::contains");
            let found = set
                .read(*oid)
                .unwrap_or_else(|err| panic!("{oid}: {err}"))
                .unwrap_or_else(|| panic!("{oid} missing from PackSet::read"));
            assert_eq!(found.0.as_str(), kind_name, "{oid}");
            assert_eq!(found.1.len() as u64, *size, "{oid}");
            assert_eq!(
                found.1,
                gitkit::git_bytes(dir, &["cat-file", kind_name, &oid.to_hex()]),
                "{oid} ({kind_name}) differs from git cat-file"
            );
        }

        let absent = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();
        if !expected.contains(&absent) {
            assert!(!set.contains(absent));
            assert!(set.read(absent).unwrap().is_none());
        }
    }

    /// 验收 1/2：`git gc --aggressive --prune=now` 产出的 pack。
    #[test]
    fn pack_set_matches_git_after_gc_aggressive() {
        if !gitkit::git_available() {
            eprintln!("git is not available: skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        gitkit::make_fixture_repo(dir);
        gitkit::git(dir, &["gc", "--aggressive", "--prune=now"]);
        assert_eq!(
            gitkit::pack_files(dir).len(),
            1,
            "gc should leave exactly one pack"
        );

        assert_pack_set_matches_git(dir);

        // 对象只剩在 pack 里：`Odb::read`（先 loose 再 pack，CONTROLLER-OWNED）必须还能读到。
        let repo = Repo::discover(dir).unwrap();
        let odb = Odb::new(&repo);
        let objects = gitkit::git_objects(dir);
        assert!(
            odb.iter_loose().unwrap().is_empty(),
            "gc should have pruned loose objects"
        );
        assert_eq!(odb.iter_all().unwrap(), expected_oids(&objects));
        for (oid, kind_name, _) in objects.iter().take(8) {
            let (kind, _) = odb.read(*oid).unwrap();
            assert_eq!(kind.as_str(), kind_name, "{oid}");
        }
    }

    /// 验收 2：`git repack -adf`（默认 window/depth）产出的 pack。
    #[test]
    fn pack_set_matches_git_after_plain_repack() {
        if !gitkit::git_available() {
            eprintln!("git is not available: skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        gitkit::make_fixture_repo(tmp.path());
        gitkit::git(tmp.path(), &["repack", "-adf"]);
        assert_pack_set_matches_git(tmp.path());
    }

    /// 验收 8：多 pack（两处含同一批 oid）结果必须一致，不许假设只有一个文件。
    #[test]
    fn pack_set_reads_multiple_packs_with_overlapping_objects() {
        if !gitkit::git_available() {
            eprintln!("git is not available: skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        gitkit::make_fixture_repo(dir);
        // pack #1：repack → OFS_DELTA。
        gitkit::git(dir, &["repack", "-adf"]);
        // pack #2：同一批对象，但 `pack-objects` 默认写 REF_DELTA。
        let revs = gitkit::git_bytes(dir, &["rev-list", "--objects", "--all"]);
        let bytes = gitkit::git_stdin(dir, &["pack-objects", "--stdout"], &revs);
        gitkit::git_stdin(dir, &["index-pack", "--stdin"], &bytes);

        let packs = gitkit::pack_files(dir);
        assert_eq!(packs.len(), 2, "expected two packs, got {packs:?}");

        let repo = Repo::discover(dir).unwrap();
        let set = PackSet::open(&repo).unwrap();
        assert_eq!(
            set.indices.len(),
            2,
            "PackSet must not assume a single pack"
        );
        assert_pack_set_matches_git(dir);

        let objects = gitkit::git_objects(dir);
        let first = PackFile::open(&packs[0].with_extension("pack")).unwrap();
        let second = PackFile::open(&packs[1].with_extension("pack")).unwrap();
        let first_idx = PackIndex::open(&packs[0]).unwrap();
        let second_idx = PackIndex::open(&packs[1]).unwrap();
        for (oid, kind_name, _) in &objects {
            let a = first.read_at(first_idx.lookup(*oid).unwrap()).unwrap();
            let b = second.read_at(second_idx.lookup(*oid).unwrap()).unwrap();
            assert_eq!(a.0, b.0, "{oid}: kind differs between packs");
            assert_eq!(a.1, b.1, "{oid}: payload differs between packs");
            assert_eq!(
                a.1,
                gitkit::git_bytes(dir, &["cat-file", kind_name, &oid.to_hex()])
            );
            let via_set = set.read(*oid).unwrap().unwrap();
            assert_eq!(via_set.0, a.0);
            assert_eq!(via_set.1, a.1);
        }
    }

    /// 验收 4：坏 `.idx`（垃圾/截断/指空 pack）必须报错，不能被静默跳过。
    #[test]
    fn pack_set_reports_corrupt_indices_instead_of_skipping_them() {
        if !gitkit::git_available() {
            eprintln!("git is not available: skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        gitkit::make_fixture_repo(dir);
        gitkit::git(dir, &["repack", "-adf"]);
        let repo = Repo::discover(dir).unwrap();
        let good_idx = gitkit::pack_files(dir)[0].clone();
        let good = fs::read(&good_idx).unwrap();
        let pack_dir = good_idx.parent().unwrap().to_path_buf();

        // (a) 多了一个完全不是 idx 的 *.idx：必须整体 Corrupt（真 pack 还在也不行）。
        let broken = pack_dir.join("pack-0000000000000000000000000000000000000000.idx");
        fs::write(&broken, b"this is not a pack index").unwrap();
        let err = PackSet::open(&repo).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "garbage idx: expected Corrupt, got {err:?}"
        );

        // (b) 截断真 idx（git 把 pack/idx 写成 0444，所以先删再写）。
        fs::remove_file(&broken).unwrap();
        fs::remove_file(&good_idx).unwrap();
        fs::write(&good_idx, &good[..good.len() - 3]).unwrap();
        let err = PackSet::open(&repo).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "truncated idx: expected Corrupt, got {err:?}"
        );
        fs::remove_file(&good_idx).unwrap();
        fs::write(&good_idx, &good).unwrap();

        // (c) idx 在、pack 不在：`open` 可以成功（idx 本身是好的），但 `read` 必须 Corrupt，
        //     绝不能退化成 ObjectNotFound。
        let oid = gitkit::git_objects(dir)[0].0;
        let pack_path = good_idx.with_extension("pack");
        let moved = pack_dir.join("pack-moved-aside.pack");
        fs::rename(&pack_path, &moved).unwrap();
        let set = PackSet::open(&repo).unwrap();
        let err = set.read(oid).unwrap_err();
        assert!(
            matches!(err, Error::Corrupt { .. }),
            "missing pack: expected Corrupt, got {err:?}"
        );
        fs::rename(&moved, &pack_path).unwrap();
        assert_pack_set_matches_git(dir);
    }

    /// 仓库里没有 pack 时：空的 PackSet（而不是报错），`Odb::read` 老实报 ObjectNotFound。
    #[test]
    fn pack_set_is_empty_when_the_repository_has_no_packs() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = Repo::init(tmp.path(), crate::repo::DEFAULT_INITIAL_BRANCH).unwrap();
        let set = PackSet::open(&repo).unwrap();
        assert!(set.indices.is_empty());
        assert!(set.iter_oids().unwrap().is_empty());

        let absent = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();
        assert!(!set.contains(absent));
        assert!(set.read(absent).unwrap().is_none());
        assert!(matches!(
            Odb::new(&repo).read(absent),
            Err(Error::ObjectNotFound(_))
        ));
    }
}
