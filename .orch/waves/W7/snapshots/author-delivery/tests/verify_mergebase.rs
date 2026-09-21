//! V7 —— 独立验证 T7：`minigit::merge::merge_base`（最佳共同祖先）。
//!
//! 纪律：
//! * **真值唯一来源是真实 git**（`git commit-tree` 造图、`git merge-base --all` /
//!   `git merge-base --is-ancestor` 判定），全部在运行时由临时仓库里的真实对象图产出；
//!   本文件不硬编码任何 oid、拓扑或期望结果。
//! * 被测对象是**公开 API** `minigit::merge::merge_base`（`Repo` → `Odb` → loose 对象，
//!   端到端），不是任何内部 helper。
//! * **反假绿**：所有 DAG 的时间戳刻意与拓扑**相反**（越早的提交时间戳越大），
//!   且 criss-cross 的两个 base 使用**同一个时间戳**。否则「按时间戳取最新共同祖先」
//!   这类错误实现会碰巧通过，断言等于恒真。

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use minigit::{Error, Oid, Repo};

// ------------------------------------------------------------------ git 测试床

/// 一个真实 git 仓库；对象都是真实 loose 对象，`mg` 经 `Odb` 读它们。
struct Git {
    dir: tempfile::TempDir,
    empty_tree: Oid,
}

impl Git {
    fn new() -> Git {
        let dir = tempfile::tempdir().expect("create tempdir");
        let mut git = Git {
            dir,
            empty_tree: Oid::zeros(),
        };
        git.must(&["init", "-q", "."]);
        // 空 tree 的 oid 由真实 git 在运行时给出（不硬编码常量）。
        let hex = git.stdin_out(&["hash-object", "-t", "tree", "-w", "--stdin"], b"");
        git.empty_tree = Oid::from_hex(hex.trim()).expect("git printed a valid empty-tree oid");
        git
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// 身份/配置/对象库环境都就地隔离，绝不 export 进共享 shell。
    fn command(&self) -> Command {
        let mut cmd = Command::new("git");
        cmd.args([
            "-c",
            "user.name=V7 verifier",
            "-c",
            "user.email=v7@example.invalid",
        ])
        .current_dir(self.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .stdin(Stdio::null());
        cmd
    }

    fn raw(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("spawn git")
    }

    fn must(&self, args: &[&str]) -> String {
        let out = self.raw(args);
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn stdin_out(&self, args: &[&str], input: &[u8]) -> String {
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn git");
        child
            .stdin
            .take()
            .expect("child stdin")
            .write_all(input)
            .expect("write git stdin");
        let out = child.wait_with_output().expect("wait for git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// `git commit-tree <空 tree> [-p …] -m …`，committer/author 时间戳显式指定。
    fn commit_tree(&self, parents: &[Oid], when: i64, message: &str) -> Oid {
        let mut args: Vec<String> = vec!["commit-tree".into(), self.empty_tree.to_hex()];
        for parent in parents {
            args.push("-p".into());
            args.push(parent.to_hex());
        }
        args.push("-m".into());
        args.push(message.into());
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();

        let date = format!("{when} +0000");
        let out = self
            .command()
            .args(&refs)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .expect("spawn git commit-tree");
        assert!(
            out.status.success(),
            "git commit-tree failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Oid::from_hex(String::from_utf8_lossy(&out.stdout).trim()).expect("commit oid")
    }

    /// 把**可能损坏**的 commit payload 交给真实 git 写进对象库（`--literally` 跳过 fsck）。
    fn write_raw_commit(&self, payload: &[u8]) -> Oid {
        let hex = self.stdin_out(
            &[
                "hash-object",
                "--literally",
                "-t",
                "commit",
                "-w",
                "--stdin",
            ],
            payload,
        );
        Oid::from_hex(hex.trim()).expect("object oid")
    }

    /// `git merge-base --all`：git 退出码 1 = 无共同祖先。
    fn merge_base_all(&self, a: Oid, b: Oid) -> Vec<Oid> {
        let out = self.raw(&["merge-base", "--all", &a.to_hex(), &b.to_hex()]);
        if out.status.code() == Some(1) {
            return Vec::new();
        }
        assert!(
            out.status.success(),
            "git merge-base --all failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| Oid::from_hex(line).expect("base oid"))
            .collect()
    }

    fn is_ancestor(&self, ancestor: Oid, descendant: Oid) -> bool {
        let out = self.raw(&[
            "merge-base",
            "--is-ancestor",
            &ancestor.to_hex(),
            &descendant.to_hex(),
        ]);
        match out.status.code() {
            Some(0) => true,
            Some(1) => false,
            other => panic!(
                "git merge-base --is-ancestor exited {other:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            ),
        }
    }

    fn exists(&self, oid: Oid) -> bool {
        self.raw(&["cat-file", "-e", &oid.to_hex()])
            .status
            .success()
    }

    /// 该提交在真实对象里的 committer 时间戳（校验构造出来的时间戳真的是我们要的）。
    fn committer_time(&self, oid: Oid) -> i64 {
        self.must(&["log", "-1", "--format=%ct", &oid.to_hex()])
            .parse()
            .expect("committer timestamp")
    }
}

/// 时间戳与拓扑**相反**（step 越大越旧）：让「按时间戳取最新共同祖先」的实现必然出错。
fn skew(step: i64) -> i64 {
    1_700_000_500 - step
}

fn hex_list<'a>(oids: impl IntoIterator<Item = &'a Oid>) -> Vec<String> {
    oids.into_iter().map(Oid::to_hex).collect()
}

// ------------------------------------------------------- 与实现无关的父表 oracle

/// 我们造图时写下的父表 + 提交顺序（真值来源，与被测代码无关）。
struct Dag {
    order: Vec<Oid>,
    parents: HashMap<Oid, Vec<Oid>>,
}

impl Dag {
    fn new() -> Dag {
        Dag {
            order: Vec::new(),
            parents: HashMap::new(),
        }
    }

    fn add(&mut self, oid: Oid, parents: Vec<Oid>) {
        self.parents.insert(oid, parents);
        self.order.push(oid);
    }
}

/// 祖先闭包（含自身），只用父表算。
fn ancestor_set(parents: &HashMap<Oid, Vec<Oid>>, start: Oid) -> HashSet<Oid> {
    let mut seen = HashSet::new();
    let mut stack = vec![start];
    while let Some(oid) = stack.pop() {
        if !seen.insert(oid) {
            continue;
        }
        if let Some(ps) = parents.get(&oid) {
            stack.extend(ps.iter().copied());
        }
    }
    seen
}

/// 极大共同祖先 = 共同祖布里「不是另一个共同祖先的严格祖先」的那些。
fn oracle_maximal_common(parents: &HashMap<Oid, Vec<Oid>>, a: Oid, b: Oid) -> HashSet<Oid> {
    let a_anc = ancestor_set(parents, a);
    let b_anc = ancestor_set(parents, b);
    let common: HashSet<Oid> = a_anc.intersection(&b_anc).copied().collect();
    let ancestors: HashMap<Oid, HashSet<Oid>> = common
        .iter()
        .map(|&c| (c, ancestor_set(parents, c)))
        .collect();
    common
        .iter()
        .filter(|&&c| {
            !common
                .iter()
                .any(|&other| other != c && ancestors[&other].contains(&c))
        })
        .copied()
        .collect()
}

/// 对一对 (a, b) 独立判定：正确性 + 最优性（真值全部来自真实 git），
/// 再用与实现无关的父表 oracle 交叉验证。返回 `merge_base` 的结果。
fn check_pair(git: &Git, repo: &Repo, dag: &Dag, a: Oid, b: Oid) -> Option<Oid> {
    let truth = git.merge_base_all(a, b);
    let got = minigit::merge::merge_base(repo, a, b)
        .unwrap_or_else(|err| panic!("merge_base({a}, {b}) 不应报错: {err}"));

    match got {
        None => assert!(
            truth.is_empty(),
            "mg 返回 None，但 git merge-base --all {a} {b} = {:?}",
            hex_list(&truth)
        ),
        Some(x) => {
            assert!(
                !truth.is_empty(),
                "mg 返回 {x}，但 git 说 {a} 与 {b} 没有共同祖先"
            );
            // (1) 正确性：结果必须是真实的共同祖先。
            assert!(
                git.is_ancestor(x, a),
                "正确性: mg 结果 {x} 不是 {a} 的祖先（{a} vs {b}，git --all = {:?}）",
                hex_list(&truth)
            );
            assert!(
                git.is_ancestor(x, b),
                "正确性: mg 结果 {x} 不是 {b} 的祖先（{a} vs {b}，git --all = {:?}）",
                hex_list(&truth)
            );
            // (2) 最优性：不得比 git 更差。
            assert!(
                truth.contains(&x),
                "最优性: mg 结果 {x} 不是极大共同祖先，git --all = {:?}（{a} vs {b}）",
                hex_list(&truth)
            );
            if truth.len() == 1 {
                assert_eq!(
                    x, truth[0],
                    "唯一最佳共同祖先时必须逐字节一致（{a} vs {b}）"
                );
            }
        }
    }

    // 独立 oracle（只用父表）：结果必须 ∈ 极大共同祖先集合。
    let oracle = oracle_maximal_common(&dag.parents, a, b);
    assert_eq!(
        oracle.is_empty(),
        got.is_none(),
        "oracle 与 mg 在「有无共同祖先」上不一致（{a} vs {b}）"
    );
    if let Some(x) = got {
        assert!(
            oracle.contains(&x),
            "oracle: {x} 不是极大共同祖先 {:?}（{a} vs {b}）",
            hex_list(&oracle)
        );
        if oracle.len() == 1 {
            assert_eq!(
                x,
                *oracle.iter().next().expect("非空"),
                "oracle: 唯一极大共同祖先时必须取它（{a} vs {b}）"
            );
        }
    }

    // 可复现、且与 (a, b) 的书写顺序无关（交付实现文档里声明的性质）。
    assert_eq!(
        minigit::merge::merge_base(repo, a, b).expect("同一对调用不应报错"),
        got,
        "同一对 (a, b) 多次调用必须可复现（{a} vs {b}）"
    );
    assert_eq!(
        minigit::merge::merge_base(repo, b, a).expect("反向调用不应报错"),
        got,
        "结果必须与 (a, b) 顺序无关（{a} vs {b}）"
    );

    got
}

fn check_all_pairs(git: &Git, repo: &Repo, dag: &Dag) {
    let order = dag.order.clone();
    for &a in &order {
        for &b in &order {
            check_pair(git, repo, dag, a, b);
        }
    }
}

fn open(git: &Git) -> Repo {
    Repo::discover(git.path()).expect("Repo::discover")
}

// --------------------------------------------------------------- 覆盖用例

/// 线性历史：全部 6×6 对与 git 对拍；结果必须是「更靠下」的提交，
/// 且**不能**是时间戳最大的 root（时间戳与拓扑相反）。
#[test]
fn linear_history_all_pairs_match_git() {
    let git = Git::new();
    let repo = open(&git);
    let mut dag = Dag::new();
    let mut parent: Option<Oid> = None;
    for step in 0..6 {
        let parents: Vec<Oid> = parent.iter().copied().collect();
        let oid = git.commit_tree(&parents, skew(step), &format!("c{step}"));
        dag.add(oid, parents);
        parent = Some(oid);
    }

    let got = minigit::merge::merge_base(&repo, dag.order[5], dag.order[2]).unwrap();
    assert_eq!(got, Some(dag.order[2]));
    assert_ne!(
        got,
        Some(dag.order[0]),
        "时间戳最大的共同祖先（root）不是最佳共同祖先"
    );

    check_all_pairs(&git, &repo, &dag);
}

/// 祖先关系两个方向 + `a == b`：期望值由真实 git 逐对确认后再比对 mg。
#[test]
fn ancestor_relation_and_identity() {
    let git = Git::new();
    let repo = open(&git);
    let mut dag = Dag::new();
    let mut parent: Option<Oid> = None;
    for step in 0..5 {
        let parents: Vec<Oid> = parent.iter().copied().collect();
        let oid = git.commit_tree(&parents, skew(step), &format!("c{step}"));
        dag.add(oid, parents);
        parent = Some(oid);
    }

    let order = dag.order.clone();
    for (i, &a) in order.iter().enumerate() {
        for (j, &b) in order.iter().enumerate() {
            // 线性链上最佳共同祖先 = 较浅的那个（下标较小者）；先让 git 确认。
            let expected = order[i.min(j)];
            assert_eq!(
                git.merge_base_all(a, b),
                vec![expected],
                "git 真值（{a} vs {b}）"
            );
            assert_eq!(
                minigit::merge::merge_base(&repo, a, b).unwrap(),
                Some(expected),
                "{a} vs {b}"
            );
            if i == j {
                assert_eq!(
                    minigit::merge::merge_base(&repo, a, b).unwrap(),
                    Some(a),
                    "a == b 时必须返回自己"
                );
            }
        }
    }
}

/// 单次合并：root → left/right → merge → after。
#[test]
fn single_merge_matches_git() {
    let git = Git::new();
    let repo = open(&git);
    let mut dag = Dag::new();
    let root = git.commit_tree(&[], skew(0), "root");
    let left = git.commit_tree(&[root], skew(1), "left");
    let right = git.commit_tree(&[root], skew(2), "right");
    let merge = git.commit_tree(&[left, right], skew(3), "merge");
    let after = git.commit_tree(&[merge], skew(4), "after");
    dag.add(root, vec![]);
    dag.add(left, vec![root]);
    dag.add(right, vec![root]);
    dag.add(merge, vec![left, right]);
    dag.add(after, vec![merge]);

    assert_eq!(git.merge_base_all(left, right), vec![root]);
    assert_eq!(
        minigit::merge::merge_base(&repo, left, right).unwrap(),
        Some(root)
    );
    assert_eq!(git.merge_base_all(merge, left), vec![left]);
    assert_eq!(
        minigit::merge::merge_base(&repo, merge, left).unwrap(),
        Some(left),
        "merge 与 left：left 是祖先，也是最佳共同祖先"
    );
    assert_eq!(git.merge_base_all(after, merge), vec![merge]);
    assert_eq!(
        minigit::merge::merge_base(&repo, after, merge).unwrap(),
        Some(merge)
    );

    check_all_pairs(&git, &repo, &dag);
}

/// criss-cross：两个最佳共同祖先，且**同戳**（避免时间戳启发式蒙对）。
#[test]
fn criss_cross_two_equals_timestamped_bases() {
    let git = Git::new();
    let repo = open(&git);
    let mut dag = Dag::new();

    //   root ──┬── a1 ──┬── a2 ──┐
    //          └── b1 ──┴── b2 ──┴── m2      m1 = merge(a1, b1)
    let root = git.commit_tree(&[], skew(0), "root");
    let a1 = git.commit_tree(&[root], skew(1), "a1");
    let b1 = git.commit_tree(&[root], skew(1), "b1"); // 与 a1 同戳
    let m1 = git.commit_tree(&[a1, b1], skew(2), "m1");
    let a2 = git.commit_tree(&[a1], skew(3), "a2");
    let b2 = git.commit_tree(&[b1], skew(4), "b2");
    let m2 = git.commit_tree(&[b2, a2], skew(5), "m2");
    dag.add(root, vec![]);
    dag.add(a1, vec![root]);
    dag.add(b1, vec![root]);
    dag.add(m1, vec![a1, b1]);
    dag.add(a2, vec![a1]);
    dag.add(b2, vec![b1]);
    dag.add(m2, vec![b2, a2]);

    let bases = git.merge_base_all(m1, m2);
    assert_eq!(
        bases.len(),
        2,
        "criss-cross 真值必须有两个最佳共同祖先: {:?}",
        hex_list(&bases)
    );
    assert_eq!(
        git.committer_time(bases[0]),
        git.committer_time(bases[1]),
        "两个 base 必须同戳（构造保证），否则断言可能是恒真的"
    );

    let got = minigit::merge::merge_base(&repo, m1, m2)
        .unwrap()
        .expect("criss-cross 有共同祖先");
    assert!(
        bases.contains(&got),
        "mg 结果 {got} 必须 ∈ git 的两个 base {:?}",
        hex_list(&bases)
    );
    assert_ne!(
        got, root,
        "被支配的 root（时间戳最大）不能作为结果 —— 这会戳穿「按时间取最新共同祖先」"
    );

    check_all_pairs(&git, &repo, &dag);
}

/// 三方合并（3 个 parent 的 octopus 提交）。
#[test]
fn three_parent_merge_matches_git() {
    let git = Git::new();
    let repo = open(&git);
    let mut dag = Dag::new();
    let root = git.commit_tree(&[], skew(0), "root");
    let p1 = git.commit_tree(&[root], skew(1), "p1");
    let p2 = git.commit_tree(&[root], skew(2), "p2");
    let p3 = git.commit_tree(&[root], skew(3), "p3");
    let octopus = git.commit_tree(&[p1, p2, p3], skew(4), "octopus");
    dag.add(root, vec![]);
    dag.add(p1, vec![root]);
    dag.add(p2, vec![root]);
    dag.add(p3, vec![root]);
    dag.add(octopus, vec![p1, p2, p3]);

    assert_eq!(git.merge_base_all(p1, p2), vec![root]);
    assert_eq!(git.merge_base_all(p2, p3), vec![root]);
    assert_eq!(git.merge_base_all(octopus, p2), vec![p2]);
    assert_eq!(
        minigit::merge::merge_base(&repo, octopus, p2).unwrap(),
        Some(p2)
    );
    assert_eq!(
        minigit::merge::merge_base(&repo, p1, p3).unwrap(),
        Some(root)
    );

    check_all_pairs(&git, &repo, &dag);
}

/// 无共同祖先：真·`git checkout --orphan` 造出的两条独立历史 + commit-tree 独立 root。
#[test]
fn unrelated_roots_have_no_common_ancestor() {
    let git = Git::new();
    let repo = open(&git);

    std::fs::write(git.path().join("a.txt"), "a\n").expect("write");
    git.must(&["add", "-A"]);
    git.must(&["commit", "-q", "-m", "A"]);
    let root_a = Oid::from_hex(&git.must(&["rev-parse", "HEAD"])).expect("oid");

    git.must(&["checkout", "-q", "--orphan", "unrelated"]);
    std::fs::write(git.path().join("b.txt"), "b\n").expect("write");
    git.must(&["add", "-A"]);
    git.must(&["commit", "-q", "-m", "B"]);
    let root_b = Oid::from_hex(&git.must(&["rev-parse", "HEAD"])).expect("oid");
    assert_ne!(root_a, root_b, "orphan 分支必须有独立的 root");

    assert!(git.merge_base_all(root_a, root_b).is_empty());
    assert_eq!(
        minigit::merge::merge_base(&repo, root_a, root_b).unwrap(),
        None
    );
    assert_eq!(
        minigit::merge::merge_base(&repo, root_b, root_a).unwrap(),
        None
    );

    let other = git.commit_tree(&[], skew(9), "third-root");
    assert_eq!(
        minigit::merge::merge_base(&repo, root_a, other).unwrap(),
        None
    );
    assert_eq!(
        minigit::merge::merge_base(&repo, other, root_b).unwrap(),
        None
    );
    assert_eq!(
        minigit::merge::merge_base(&repo, other, other).unwrap(),
        Some(other)
    );
}

/// ~200 提交、含多次合并的多分支 DAG：抽样对拍 + 性能必须秒级。
#[test]
fn large_dag_200_commits_is_fast_and_correct() {
    let git = Git::new();
    let repo = open(&git);

    // 确定性 xorshift：让「随机」DAG 可复现。
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, upper: usize) -> usize {
            (self.next() % upper as u64) as usize
        }
    }

    let count = 200usize;
    let mut rng = Rng(0xC0FFEE5EED1234);
    let mut dag = Dag::new();
    for index in 0..count {
        let parents: Vec<Oid> = if index == 0 {
            Vec::new()
        } else if index > 1 && rng.next() % 100 < 35 {
            let first = dag.order[rng.below(index)];
            let second = dag.order[rng.below(index)];
            if first == second {
                vec![first]
            } else {
                vec![first, second]
            }
        } else {
            vec![dag.order[rng.below(index)]]
        };
        // 时间戳与拓扑无关（真实历史也有时钟 skew）。
        let when = 1_700_000_000 + (rng.next() % 4096) as i64;
        let oid = git.commit_tree(&parents, when, &format!("c{index}"));
        dag.add(oid, parents);
    }
    assert_eq!(dag.order.len(), count);

    // 抽样：与真实 git 对拍（每条 3 次 mg 调用 + 3 次 git 调用，200 提交的图）。
    let mut pick = Rng(0xBEEF_0F1E);
    for _ in 0..40 {
        let a = dag.order[pick.below(count)];
        let b = dag.order[pick.below(count)];
        check_pair(&git, &repo, &dag, a, b);
    }

    // 性能：200 对单次调用。
    let samples: Vec<(Oid, Oid)> = (0..200)
        .map(|_| (dag.order[pick.below(count)], dag.order[pick.below(count)]))
        .collect();
    let started = Instant::now();
    let mut worst = Duration::ZERO;
    for &(a, b) in &samples {
        let t0 = Instant::now();
        minigit::merge::merge_base(&repo, a, b).expect("no error");
        worst = worst.max(t0.elapsed());
    }
    let total = started.elapsed();
    eprintln!("200-commit DAG: 200 calls in {total:?} (worst single call {worst:?})");
    assert!(
        worst < Duration::from_secs(1),
        "单次调用 {worst:?} 必须秒级返回（200 提交 DAG）"
    );
    assert!(
        total < Duration::from_secs(5),
        "200 对总计 {total:?} 必须秒级返回（200 提交 DAG）"
    );
}

/// 端到端前提：对象确实是 loose 且不存在 packfile —— 因此上面的用例只能经
/// `Repo` → `Odb::read_object` → `loose::read_loose` 读到它们（T5 已完成，这条路径真的被覆盖）。
#[test]
fn objects_are_loose_so_the_odb_read_path_is_covered() {
    let git = Git::new();
    let repo = open(&git);
    let root = git.commit_tree(&[], skew(0), "root");

    let loose = repo.objects_dir().join(root.loose_rel_path());
    assert!(
        loose.is_file(),
        "commit 必须是 loose 对象（{}）",
        loose.display()
    );
    let packs: Vec<String> = std::fs::read_dir(repo.objects_dir().join("pack"))
        .expect("objects/pack")
        .map(|entry| {
            entry
                .expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.ends_with(".pack") || name.ends_with(".idx"))
        .collect();
    assert!(packs.is_empty(), "不应存在 packfile: {packs:?}");

    assert_eq!(
        minigit::merge::merge_base(&repo, root, root).unwrap(),
        Some(root)
    );
}

/// 反例：不存在的 oid → `Err(ObjectNotFound)`（不是 panic、不是 Corrupt）。
#[test]
fn nonexistent_oid_is_object_not_found() {
    let git = Git::new();
    let repo = open(&git);
    let root = git.commit_tree(&[], skew(0), "root");
    let tip = git.commit_tree(&[root], skew(1), "tip");
    let missing = Oid::hash_object("commit", b"V7: this object was never written anywhere");
    assert!(!git.exists(missing), "构造的 oid 必须真的不存在");

    for (a, b) in [(tip, missing), (missing, tip), (missing, missing)] {
        match minigit::merge::merge_base(&repo, a, b) {
            Err(Error::ObjectNotFound(oid)) => assert_eq!(oid, missing),
            other => panic!("期望 Err(ObjectNotFound({missing}))，实际 {other:?}"),
        }
    }
}

/// 反例：损坏的 commit 对象 → `Err(Corrupt)` 而不是 panic。
#[test]
fn corrupt_commit_is_corrupt_not_a_panic() {
    let git = Git::new();
    let repo = open(&git);
    let good = git.commit_tree(&[], skew(0), "good");
    let tree = git.empty_tree.to_hex();

    let payloads: Vec<Vec<u8>> = vec![
        // 签名行缺 `<email>`。
        format!("tree {tree}\nauthor no-email-here\ncommitter no-email-here\n\nbroken\n")
            .into_bytes(),
        // 缺 tree 头。
        b"author A <a@b> 1 +0000\ncommitter A <a@b> 1 +0000\n\nno tree\n".to_vec(),
        // tree 不是合法 oid。
        b"tree zz\nauthor A <a@b> 1 +0000\ncommitter A <a@b> 1 +0000\nx\n".to_vec(),
        // 头部与 message 之间缺空行。
        format!("tree {tree}\nauthor A <a@b> 1 +0000\ncommitter A <a@b> 1 +0000\n").into_bytes(),
    ];

    for (index, payload) in payloads.iter().enumerate() {
        let bad = git.write_raw_commit(payload);
        assert_eq!(
            bad,
            Oid::hash_object("commit", payload),
            "坏对象 #{index} 必须以真实内容哈希入库"
        );
        for (a, b) in [(bad, bad), (good, bad), (bad, good)] {
            match minigit::merge::merge_base(&repo, a, b) {
                Err(Error::Corrupt { .. }) => {}
                other => panic!("坏对象 #{index}（{a} vs {b}）应得 Err(Corrupt)，实际 {other:?}"),
            }
        }
    }
}
