#!/usr/bin/env bash
# V16 边界探针（T16 验证者）：把两处「mg 与真实 git 不一致」写成可复跑证据。
#
# 两处都**不是** D1（嵌套路径快进）的症状，也不在本任务写作用域内，
# 但按任务书「发现任务书/前提有误要写明」的要求，必须给出最小复现与真实命令输出。
#
#   边界 1：本地改动**不会**被快进覆盖时，真实 git 允许快进，mg 一律拒绝。
#           来源：src/cli/merge.rs::merge_into 的 `worktree.status` 前置守卫（T10 代码，未改动）。
#   边界 2：`mg pull` 冲突标记的 theirs 标签用 ref 名，`git pull` 用 fetch 到的 oid。
#           来源：pull 把目标 ref 名交给 merge.rs；git pull 合并的是 FETCH_HEAD。
#           注：`git merge <ref>` 与 mg **完全一致**（同一仓库实测）。
#
# 用法：bash .orch/waves/W4/T16-merge-ff/verify-scratch/probe-boundaries.sh
# 环境：需要真实 git 与已构建的 target/debug/mg。

set -uo pipefail

root="$(cd "$(dirname "$0")/../../../../.." && pwd)"
MG="${MG:-$root/target/debug/mg}"
R="$(mktemp -d "${TMPDIR:-/tmp}/v16-boundaries.XXXXXX")"
trap 'rm -rf "$R"' EXIT

GIT=(
  git -c core.pager=cat
  -c protocol.file.allow=always
)
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 LC_ALL=C
export GIT_AUTHOR_NAME=V16 GIT_AUTHOR_EMAIL=v16@example.com
export GIT_COMMITTER_NAME=V16 GIT_COMMITTER_EMAIL=v16@example.com
export GIT_AUTHOR_DATE="1700000000 +0000" GIT_COMMITTER_DATE="1700000000 +0000"
export GIT_MERGE_AUTOEDIT=no

echo "mg = $MG"
"$MG" --version

# ---------------------------------------------------------------------------
echo
echo "=== 边界 1：快进 + 本地改动在「不会被覆盖」的文件上 ==="
b1="$R/b1"
mkdir -p "$b1"; cd "$b1" || exit 1
"${GIT[@]}" init -q -b main
mkdir -p dir; printf 'beta\n' > dir/b.txt; printf 'only\n' > only.txt
"${GIT[@]}" add -A; "${GIT[@]}" commit -qm base
"${GIT[@]}" checkout -q -b side
printf 'beta side\n' > dir/b.txt; printf 's\n' > s.txt
"${GIT[@]}" add -A; "${GIT[@]}" commit -qm side
"${GIT[@]}" checkout -q main
# 本地改动只碰 only.txt（side 完全没动它 → git 允许快进）
printf 'local only\n' > only.txt
cp -a "$b1" "$R/b1-mg"

echo "--- 真实 git：merge side"
( cd "$b1" && "${GIT[@]}" merge side ); echo "git exit=$?"
echo "--- mg：merge side"
( cd "$R/b1-mg" && "$MG" merge side ); echo "mg exit=$?"
echo "--- HEAD 对比"
echo "git HEAD = ${GIT[0]} -C $b1 ... $("${GIT[@]}" -C "$b1" rev-parse HEAD)"
echo "mg  HEAD = $("${GIT[@]}" -C "$R/b1-mg" rev-parse HEAD)"
echo "side     = $("${GIT[@]}" -C "$b1" rev-parse refs/heads/side)"
echo "git worktree: $("${GIT[@]}" -C "$b1" status --porcelain | tr '\n' '|') incoming=[$(cat "$b1/s.txt" 2>/dev/null)] only=[$(cat "$b1/only.txt")]"
echo "mg  worktree: $("${GIT[@]}" -C "$R/b1-mg" status --porcelain | tr '\n' '|') incoming=[$(cat "$R/b1-mg/s.txt" 2>/dev/null)] only=[$(cat "$R/b1-mg/only.txt")]"

# ---------------------------------------------------------------------------
echo
echo "=== 边界 2：分叉 pull 冲突时 theirs 标记标签 ==="
b2="$R/b2"
"${GIT[@]}" init -q --bare -b main "$b2-remote.git"
mkdir -p "$b2-seed"; cd "$b2-seed" || exit 1
"${GIT[@]}" init -q -b main
printf 'a\nb\nc\nd\ne\n' > f.txt
"${GIT[@]}" add -A; "${GIT[@]}" commit -qm base
"${GIT[@]}" remote add origin "$b2-remote.git"
"${GIT[@]}" push -q -u origin main
cd "$R" || exit 1
"${GIT[@]}" clone -q "$b2-remote.git" "$b2-local"
cd "$b2-local" || exit 1
printf 'a\nLOCAL\nc\nd\ne\n' > f.txt
"${GIT[@]}" add -A; "${GIT[@]}" commit -qm local-diverge
cp -a "$b2-local" "$b2-gitpull"
cp -a "$b2-local" "$b2-mgmerge"
cp -a "$b2-local" "$b2-mgpull"
cd "$b2-seed" || exit 1
printf 'a\nREMOTE\nc\nd\ne\n' > f.txt
"${GIT[@]}" add -A; "${GIT[@]}" commit -qm remote-advance
"${GIT[@]}" push -q origin main

( cd "$b2-gitpull" && "${GIT[@]}" -c pull.rebase=false pull --no-edit >/dev/null 2>&1 ); echo "git pull    exit=$?"
( cd "$b2-mgpull"  && "$MG" pull >/dev/null 2>&1 ); echo "mg  pull    exit=$?"
( cd "$b2-mgmerge" && "${GIT[@]}" fetch -q origin && "$MG" merge refs/remotes/origin/main >/dev/null 2>&1 ); echo "mg  merge   exit=$?"
for d in b2-gitpull b2-mgpull b2-mgmerge; do
  echo "--- $d: [$(cd "$R/$d" && tr '\n' '|' < f.txt)] porcelain=[$("${GIT[@]}" -C "$R/$d" status --porcelain | tr '\n' '|')]"
done
echo "--- 真值：git merge <ref> 的标签（与 mg 相同）"
echo "[$(cd "$R/b2-mgmerge" && tr '\n' '|' < f.txt)]"
