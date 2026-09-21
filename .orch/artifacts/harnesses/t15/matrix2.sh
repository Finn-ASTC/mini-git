#!/usr/bin/env bash
# Corrected matrix: nested-path behaviour of merge / switch / checkout / reset.
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c gc.auto=0"
W=/tmp/t15/matrix2; rm -rf "$W"; mkdir -p "$W"

note() { echo "    $*"; }
status_of() { $G status --porcelain | tr '\n' '|'; }

echo "=== A. fast-forward merge, FLAT repo (feature adds top-level file)"
R=$W/A; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; printf 'alpha\n' > a.txt; $MG add . >/dev/null; $MG commit -m c1 >/dev/null
$MG switch -c side main >/dev/null; printf 'side\n' > s.txt; $MG add s.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG switch main >/dev/null
$MG merge side >/tmp/t15/matrix2/A.out 2>/tmp/t15/matrix2/A.err; note "exit=$? stdout=$(cat /tmp/t15/matrix2/A.out|tr '\n' '|') stderr=$(cat /tmp/t15/matrix2/A.err|tr '\n' '|')"
note "files=$(find . -type f -not -path './.git/*'|sort|tr '\n' ' ') status=$(status_of) log=$($G log --oneline|tr '\n' '|')"

echo "=== B. fast-forward merge, NESTED repo (repo contains dir/b.txt)"
R=$W/B; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; printf 'alpha\n' > a.txt; $MG add . >/dev/null; $MG commit -m c1 >/dev/null
$MG switch -c side main >/dev/null; printf 'side\n' > s.txt; $MG add s.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG switch main >/dev/null
$MG merge side >/tmp/t15/matrix2/B.out 2>/tmp/t15/matrix2/B.err; note "exit=$? stdout=$(cat /tmp/t15/matrix2/B.out|tr '\n' '|') stderr=$(cat /tmp/t15/matrix2/B.err|tr '\n' '|')"
note "log=$($G log --oneline|tr '\n' '|')"

echo "=== C. fast-forward merge, NESTED repo, change inside dir/"
R=$W/C; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; $MG add . >/dev/null; $MG commit -m c1 >/dev/null
$MG switch -c side main >/dev/null; printf 'beta v2\n' > dir/b.txt; $MG add dir/b.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG switch main >/dev/null
$MG merge side >/tmp/t15/matrix2/C.out 2>/tmp/t15/matrix2/C.err; note "exit=$? stdout=$(cat /tmp/t15/matrix2/C.out|tr '\n' '|') stderr=$(cat /tmp/t15/matrix2/C.err|tr '\n' '|')"
note "dir/b.txt=$(cat dir/b.txt|tr '\n' '|')"

echo "=== D. clean (non-ff) auto-merge, NESTED repo (divergent top-level files)"
R=$W/D; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; $MG add . >/dev/null; $MG commit -m c1 >/dev/null
$MG switch -c side main >/dev/null; printf 'side\n' > s.txt; $MG add s.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG switch main >/dev/null; printf 'main\n' > m.txt; $MG add m.txt >/dev/null; $MG commit -m c3 >/dev/null
$MG merge side >/tmp/t15/matrix2/D.out 2>/tmp/t15/matrix2/D.err; note "exit=$? stdout=$(cat /tmp/t15/matrix2/D.out|tr '\n' '|') stderr=$(cat /tmp/t15/matrix2/D.err|tr '\n' '|')"
note "files=$(find . -type f -not -path './.git/*'|sort|tr '\n' ' ') status=$(status_of) log=$($G log --oneline|tr '\n' '|') parents=$($G rev-list --parents -1 HEAD|tr ' ' '|')"

echo "=== E. conflict inside a NESTED file (dir/b.txt)"
R=$W/E; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; $MG add . >/dev/null; $MG commit -m c1 >/dev/null
$MG switch -c side main >/dev/null; printf 'side version\n' > dir/b.txt; $MG add dir/b.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG switch main >/dev/null; printf 'main version\n' > dir/b.txt; $MG add dir/b.txt >/dev/null; $MG commit -m c3 >/dev/null
$MG merge side >/tmp/t15/matrix2/E.out 2>/tmp/t15/matrix2/E.err; note "merge exit=$? stderr=$(cat /tmp/t15/matrix2/E.err|tr '\n' '|')"
note "status=$(status_of)"; note "dir/b.txt: $(cat dir/b.txt|tr '\n' '|')"
printf 'resolved\n' > dir/b.txt; $MG add dir/b.txt >/dev/null; $MG commit -m merged >/dev/null 2>&1
note "after resolve: commit exit=$? log=$($G log --oneline|tr '\n' '|') status=$(status_of) fsck=$($G fsck --no-progress 2>&1|tr '\n' '|')"

echo "=== F. switch that changes a NESTED file"
R=$W/F; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; $MG add . >/dev/null; $MG commit -m c1 >/dev/null
$MG switch -c side main >/dev/null; printf 'beta v2\n' > dir/b.txt; $MG add dir/b.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG switch main >/tmp/t15/matrix2/F1.out 2>/tmp/t15/matrix2/F1.err; note "switch main exit=$? stderr=$(cat /tmp/t15/matrix2/F1.err|tr '\n' '|') dir/b.txt=$(cat dir/b.txt|tr '\n' '|')"
$MG switch side >/tmp/t15/matrix2/F2.out 2>/tmp/t15/matrix2/F2.err; note "switch side exit=$? stderr=$(cat /tmp/t15/matrix2/F2.err|tr '\n' '|') dir/b.txt=$(cat dir/b.txt|tr '\n' '|')"

echo "=== G. checkout of a NESTED-differing branch"
R=$W/G; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; $MG add . >/dev/null; $MG commit -m c1 >/dev/null
$MG switch -c side main >/dev/null; printf 'beta v2\n' > dir/b.txt; $MG add dir/b.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG checkout main >/tmp/t15/matrix2/G.out 2>/tmp/t15/matrix2/G.err; note "checkout main exit=$? stderr=$(cat /tmp/t15/matrix2/G.err|tr '\n' '|') dir/b.txt=$(cat dir/b.txt|tr '\n' '|') HEAD=$($G symbolic-ref -q HEAD || $G rev-parse HEAD)"

echo "=== H. reset --hard that changes a NESTED file"
R=$W/H; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; $MG add . >/dev/null; $MG commit -m c1 >/dev/null
printf 'beta v2\n' > dir/b.txt; $MG add dir/b.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG reset --hard $($G rev-parse HEAD~1) >/tmp/t15/matrix2/H.out 2>/tmp/t15/matrix2/H.err; note "reset --hard exit=$? stdout=$(cat /tmp/t15/matrix2/H.out|tr '\n' '|') stderr=$(cat /tmp/t15/matrix2/H.err|tr '\n' '|') dir/b.txt=$(cat dir/b.txt|tr '\n' '|') status=$(status_of)"

echo "=== I. checkout <rev> -- <path> restore"
R=$W/I; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; $MG add . >/dev/null; $MG commit -m c1 >/dev/null
printf 'beta v2\n' > dir/b.txt
$MG checkout HEAD -- dir/b.txt >/tmp/t15/matrix2/I.out 2>/tmp/t15/matrix2/I.err; note "checkout HEAD -- dir/b.txt exit=$? stderr=$(cat /tmp/t15/matrix2/I.err|tr '\n' '|') content=$(cat dir/b.txt|tr '\n' '|') status=$(status_of)"
