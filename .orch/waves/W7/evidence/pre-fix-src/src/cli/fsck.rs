//! `mg fsck` —— **T15（omp）**。
//!
//! 检查链（结论方向与真实 `git fsck` 对齐，不追求逐字节相同）：
//! 1. **refs**：`refs/**` 与 `HEAD` 指向的对象必须存在（`RefStore::list` + `Odb::read`）；
//! 2. **可达对象**：从全部 ref 出发遍历 commit → tree → blob/tag：
//!    * 对象的 `Oid::hash_object(kind, payload)` 必须等于它的 oid；
//!    * tree 条目：mode 合法、名字非空且不含 `/`、不是 `.`/`..`、无重名、按 git 规则排序，
//!      目标对象存在且类型与 mode 相符（tree ↔ 目录、其余 ↔ blob）；
//!    * commit：`tree` / `parent` 可解析且类型正确、`author` / `committer` 行格式正确；
//!    * tag：`object` / `type` / `tag` 行存在，object 存在且实际类型与 `type` 一致；
//! 3. **index**：trailer（`Index::read` 已校验）+ 每个 stage 条目的 oid 在对象库里存在；
//! 4. **pack**：`.idx` 的对象数必须等于 `.pack` 头里的 count，且每个对象都能解开、
//!    重算出的 oid 与 `.idx` 里的记录一致；`.pack` 与 `.idx` 必须成对。
//!
//! **dangling 对象（存在但不可达）不是错误** —— 与 git 一致（git 只把它们列成
//! `dangling <type> <oid>` 而不改退出码）。
//!
//! `--full` 的语义跟着真实 git：`git fsck --full` 表示「loose 与 pack/alternates 都查」，
//! 而且 **自 git 2.x 起就是默认行为**（`--no-full` 才关掉；见 git 2.55 的 `git fsck` 手册）。
//! mg 的默认口径已经是「loose + pack 全查、pack 内每个对象逐一重算 oid」，因此 `--full`
//! 与默认等价，参数仅用于对齐 CLI 表面。
//!
//! 已知限制（v1）：
//! * 不做 `git fsck` 的 reflog / `objects/info/alternates` / commit-graph 检查；
//! * gitlink（`160000`）条目在 mg 里是 `Unsupported`：含子模块的仓库会被报成「不支持」
//!   而不是「损坏」；DIRC v4（split index、index.version=4）同样超出 v1 范围。

use std::collections::{BTreeSet, VecDeque};
use std::io::Write;

use crate::error::{Error, Result};
use crate::index::Index;
use crate::object::{Kind, Object, Tree};
use crate::odb::pack::{PackFile, PackIndex};
use crate::odb::{loose, Odb, PackSet};
use crate::oid::Oid;
use crate::refs::{Head, RefStore};
use crate::repo::Repo;

/// 待查对象：`(oid, 期望类型, 上下文)`。期望类型为 `None` 时只要求存在且自洽
/// （ref 直接指向 blob/tree 在 git 里并不算错，所以 ref 顶点不设类型要求）。
type Queue = VecDeque<(Oid, Option<Kind>, String)>;

#[derive(Default)]
struct Report {
    problems: Vec<String>,
    refs: usize,
    objects: usize,
    pack_objects: usize,
    index_entries: usize,
}

impl Report {
    fn problem(&mut self, message: impl Into<String>) {
        self.problems.push(message.into());
    }
}

pub fn run(full: bool) -> Result<()> {
    let repo = crate::cli::open_repo()?;
    let report = check(&repo, full);

    if report.problems.is_empty() {
        // 与 git 同向：干净的仓库不打印任何错误、退出码 0。
        return Ok(());
    }
    let stderr = std::io::stderr();
    let mut out = stderr.lock();
    for problem in &report.problems {
        writeln!(out, "error: {problem}")?;
    }
    out.flush()?;
    Err(Error::Other(format!(
        "fsck found {} problem(s) in {} (checked {} ref(s), {} reachable object(s), {} packed object(s), {} index entry(ies))",
        report.problems.len(),
        repo.git_dir().display(),
        report.refs,
        report.objects,
        report.pack_objects,
        report.index_entries
    )))
}

fn check(repo: &Repo, full: bool) -> Report {
    // `--full` 与默认等价（见模块文档）：mg 一律把 loose 与 pack 全查一遍。
    let _ = full;

    let mut report = Report::default();
    let store = RefStore::new(repo);
    let odb = Odb::new(repo);

    // 对象存在性索引（index 校验用）：loose 走路径判断，packed 走 `.idx` 集合。
    let packed: BTreeSet<Oid> = match PackSet::open(repo) {
        Ok(set) => set
            .indices
            .iter()
            .flat_map(|index| index.iter_oids().iter().copied())
            .collect(),
        Err(err) => {
            report.problem(format!("cannot read the pack directory: {err}"));
            BTreeSet::new()
        }
    };

    let mut tips: Vec<(String, Oid)> = Vec::new();
    match store.list() {
        Ok(refs) => {
            report.refs = refs.len();
            tips.extend(refs);
        }
        Err(err) => report.problem(format!("cannot list refs: {err}")),
    }
    match store.read_head() {
        Ok(Head::Detached(oid)) => tips.push(("HEAD".to_string(), oid)),
        Ok(Head::Attached(name)) => match store.resolve(&name) {
            Ok(oid) => tips.push((format!("HEAD -> {name}"), oid)),
            // HEAD 指向尚未创建的分支：git 只打印「unborn branch」提示，不是错误。
            Err(Error::RefNotFound(_)) => {}
            Err(err) => report.problem(format!("cannot resolve HEAD -> {name}: {err}")),
        },
        Err(Error::RefNotFound(_)) => {}
        Err(err) => report.problem(format!("cannot read HEAD: {err}")),
    }

    reachable(&odb, tips, &mut report);
    check_index(repo, &packed, &mut report);
    check_packs(repo, &mut report);
    report
}

/// 从全部 ref 出发做可达性遍历 + 逐对象校验。
fn reachable(odb: &Odb, tips: Vec<(String, Oid)>, report: &mut Report) {
    let mut queue: Queue = tips
        .into_iter()
        .map(|(name, oid)| (oid, None, format!("ref {name}")))
        .collect();
    let mut seen: BTreeSet<Oid> = BTreeSet::new();

    while let Some((oid, expected, context)) = queue.pop_front() {
        if !seen.insert(oid) {
            continue;
        }
        let (kind, payload) = match odb.read(oid) {
            Ok(found) => found,
            Err(err) => {
                report.problem(format!("{context}: {oid} is missing or unreadable ({err})"));
                continue;
            }
        };
        report.objects += 1;
        if let Some(expected) = expected {
            if expected != kind {
                report.problem(format!("{context}: {oid} is a {kind}, expected {expected}"));
                continue;
            }
        }
        let actual = Oid::hash_object(kind.as_str(), &payload);
        if actual != oid {
            report.problem(format!("{context}: {oid} hashes to {actual}"));
        }
        match Object::decode(kind, &payload) {
            Ok(object) => walk(object, oid, report, &mut queue),
            Err(err) => report.problem(format!("{context}: cannot decode {kind} {oid}: {err}")),
        }
    }
}

fn walk(object: Object, oid: Oid, report: &mut Report, queue: &mut Queue) {
    match object {
        Object::Blob(_) => {}
        Object::Tree(tree) => check_tree(&tree, oid, report, queue),
        Object::Commit(commit) => {
            queue.push_back((
                commit.tree,
                Some(Kind::Tree),
                format!("tree of commit {oid}"),
            ));
            for (index, parent) in commit.parents.iter().enumerate() {
                queue.push_back((
                    *parent,
                    Some(Kind::Commit),
                    format!("parent #{} of commit {oid}", index + 1),
                ));
            }
        }
        Object::Tag(tag) => {
            queue.push_back((tag.object, Some(tag.kind), format!("object of tag {oid}")));
        }
    }
}

fn check_tree(tree: &Tree, oid: Oid, report: &mut Report, queue: &mut Queue) {
    let mut names: BTreeSet<&[u8]> = BTreeSet::new();
    for entry in tree.entries() {
        let name = entry.name.as_slice();
        let shown = String::from_utf8_lossy(name);
        if name.is_empty() {
            report.problem(format!("tree {oid}: an entry has an empty name"));
        }
        if name.contains(&b'/') {
            report.problem(format!("tree {oid}: entry {shown:?} contains '/'"));
        }
        if name == b"." || name == b".." {
            report.problem(format!(
                "tree {oid}: entry {shown:?} is a relative path component"
            ));
        }
        if !names.insert(name) {
            report.problem(format!("tree {oid}: duplicate entry {shown:?}"));
        }
        let expected = if entry.mode.is_tree() {
            Kind::Tree
        } else {
            Kind::Blob
        };
        queue.push_back((
            entry.oid,
            Some(expected),
            format!("tree {oid} entry {shown:?}"),
        ));
    }

    // 编码是按 git 的规范序（`base_name_compare`）排的，所以「重排后不相等」= 顺序不合法。
    let mut canonical = tree.entries().to_vec();
    let _ = Tree::sort_entries(&mut canonical);
    if canonical.as_slice() != tree.entries() {
        report.problem(format!(
            "tree {oid}: entries are not in the canonical git order"
        ));
    }
}

fn check_index(repo: &Repo, packed: &BTreeSet<Oid>, report: &mut Report) {
    let index = match Index::read(repo) {
        Ok(index) => index,
        // 不存在 → 空 index（不是错误）；trailer/格式坏了 → Index::read 报 Corrupt。
        Err(err) => {
            report.problem(format!("cannot read the index: {err}"));
            return;
        }
    };
    report.index_entries = index.len();
    for entry in &index.entries {
        if !exists(repo, packed, entry.oid) {
            report.problem(format!(
                "index entry {}: object {} is missing",
                String::from_utf8_lossy(&entry.path),
                entry.oid
            ));
        }
    }
}

fn exists(repo: &Repo, packed: &BTreeSet<Oid>, oid: Oid) -> bool {
    packed.contains(&oid) || loose::loose_path(repo, oid).is_file()
}

fn check_packs(repo: &Repo, report: &mut Report) {
    let dir = repo.objects_dir().join("pack");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        // 没有 pack 目录 = 没有 pack，不是错误。
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            report.problem(format!("cannot read {}: {err}", dir.display()));
            return;
        }
    };

    let mut indices = Vec::new();
    let mut packs = BTreeSet::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("idx") => indices.push(path),
            Some("pack") => {
                packs.insert(path);
            }
            _ => {}
        }
    }
    indices.sort();

    for index_path in &indices {
        let pack_path = index_path.with_extension("pack");
        packs.remove(&pack_path);
        let index = match PackIndex::open(index_path) {
            Ok(index) => index,
            Err(err) => {
                report.problem(format!("{}: {err}", index_path.display()));
                continue;
            }
        };
        if !pack_path.is_file() {
            report.problem(format!(
                "{}: has no matching .pack file",
                index_path.display()
            ));
            continue;
        }
        let pack = match PackFile::open(&pack_path) {
            Ok(pack) => pack,
            Err(err) => {
                report.problem(format!("{}: {err}", pack_path.display()));
                continue;
            }
        };
        if pack.object_count() as usize != index.len() {
            report.problem(format!(
                "{}: the header declares {} object(s) but {} lists {}",
                pack_path.display(),
                pack.object_count(),
                index_path.display(),
                index.len()
            ));
        }
        for oid in index.iter_oids() {
            let Some(offset) = index.lookup(*oid) else {
                continue;
            };
            match pack.read_at(offset) {
                Ok((kind, payload)) => {
                    report.pack_objects += 1;
                    let actual = Oid::hash_object(kind.as_str(), &payload);
                    if actual != *oid {
                        report.problem(format!(
                            "{}: the object at offset {offset} hashes to {actual}, but the index maps it to {oid}",
                            pack_path.display()
                        ));
                    }
                }
                Err(err) => report.problem(format!(
                    "{}: cannot read the object at offset {offset} ({oid}): {err}",
                    pack_path.display()
                )),
            }
        }
    }

    for orphan in packs {
        report.problem(format!(
            "{}: .pack without a matching .idx",
            orphan.display()
        ));
    }
}
