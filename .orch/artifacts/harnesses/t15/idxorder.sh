#!/usr/bin/env bash
# Is mg's index sorted the way git's is?
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c gc.auto=0"
W=/tmp/t15/idxorder; rm -rf "$W"; mkdir -p "$W"

echo "=== mg add . then git ls-files --stage"
R=$W/mg; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null
printf 'alpha\n' > a.txt; mkdir dir; printf 'beta\n' > dir/b.txt; printf 'gamma\n' > c.txt
$MG add .
echo "-- git ls-files --stage:"; $G ls-files --stage
echo "-- git status --porcelain:"; $G status --porcelain
echo "-- git ls-files (no stage):"; $G ls-files

echo "=== same fixture with real git add (reference order)"
R=$W/git; mkdir -p "$R"; cd "$R"
$G init -q .
printf 'alpha\n' > a.txt; mkdir dir; printf 'beta\n' > dir/b.txt; printf 'gamma\n' > c.txt
$G add .
echo "-- git ls-files --stage:"; $G ls-files --stage

echo "=== mg ls-files? (mg has no ls-files) -> check mg status order"
R=$W/mg2; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null
printf 'alpha\n' > a.txt; mkdir dir; printf 'beta\n' > dir/b.txt; printf 'gamma\n' > c.txt
$G add .   # let real git write the index, then ask mg to read/rewrite it
echo "-- git wrote it; git ls-files --stage:"; $G ls-files --stage
$MG add .  # mg rewrites the same index
echo "-- after mg add .; git ls-files --stage:"; $G ls-files --stage

echo "=== does git complain about mg's index order?"
R=$W/mg; cd "$R"
$G fsck --no-progress; echo "   fsck exit=$?"
$G write-tree; echo "   write-tree exit=$?"
echo "-- git commit in mg's repo (does git reorder?):"
$G -c user.name=A -c user.email=a@e.com commit -qm x 2>&1 | sed 's/^/   /'
$G ls-files --stage
