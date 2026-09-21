#!/usr/bin/env bash
# Reproduce the test's exact sequence (hash-object before add) and inspect index order.
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c gc.auto=0 -c init.defaultBranch=main"
W=/tmp/t15/idxorder2; rm -rf "$W"; mkdir -p "$W"; cd "$W"
$MG init . >/dev/null
printf 'alpha\n' > a.txt; mkdir dir; printf 'beta\n' > dir/b.txt; printf 'gamma\n' > c.txt
echo "-- mg hash-object a.txt / -w"; $MG hash-object a.txt; $MG hash-object -w a.txt
echo "-- mg add ."
$MG add .
echo "-- git ls-files --stage:"; $G ls-files --stage
echo "-- git status --porcelain:"; $G status --porcelain
echo "-- mg status --porcelain:"; $MG status --porcelain
echo "-- expected sorted order (git add in a parallel repo):"
P=$W/par; mkdir -p "$P"; cd "$P"; $G init -q .; printf 'alpha\n' > a.txt; mkdir dir; printf 'beta\n' > dir/b.txt; printf 'gamma\n' > c.txt; $G add .; $G ls-files --stage
