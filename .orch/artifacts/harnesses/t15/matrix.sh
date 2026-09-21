#!/usr/bin/env bash
# Focused matrix: which merge/switch/checkout paths fail with nested paths?
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
W=/tmp/t15/matrix; rm -rf "$W"; mkdir -p "$W"

run() { # run <name> <nested|flat> <mode>
  local name="$1" nested="$2" mode="$3"
  local R=$W/$name; mkdir -p "$R"; cd "$R"
  $MG init . >/dev/null
  if [ "$nested" = nested ]; then mkdir dir; printf 'beta\n' > dir/b.txt; fi
  printf 'alpha\n' > a.txt
  $MG add . >/dev/null; $MG commit -m c1 >/dev/null
  $MG branch side >/dev/null
  # a change on the side branch
  if [ "$nested" = nested ] && [ "$mode" = nested_change ]; then
    printf 'beta v2\n' > dir/b.txt; $MG add dir/b.txt >/dev/null
  else
    printf 'side\n' > s.txt; $MG add s.txt >/dev/null
  fi
  $MG commit -m c2 >/dev/null
  $MG switch main >/dev/null 2>&1
  echo "=== $name (nested=$nested mode=$mode)"
  echo "    repo files before merge: $(find . -type f -not -path './.git/*' | sort | tr '\n' ' ')"
  $MG merge side > /tmp/t15/matrix/$name.out 2>/tmp/t15/matrix/$name.err
  echo "    mg merge exit=$?  stdout=$(cat /tmp/t15/matrix/$name.out | tr '\n' '|')  stderr=$(cat /tmp/t15/matrix/$name.err | tr '\n' '|')"
  echo "    files after: $(find . -type f -not -path './.git/*' | sort | tr '\n' ' ')"
  echo "    git status: $(git -c gc.auto=0 status --porcelain | tr '\n' '|')"
}

run ff_flat flat side_only
run ff_nested nested side_only
run ff_nested_files nested nested_change

echo
echo "=== switch to a branch whose NESTED file differs (materialize with nested tree)"
R=$W/switch_nested; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; printf 'alpha\n' > a.txt
$MG add . >/dev/null; $MG commit -m c1 >/dev/null
$MG switch -c side >/dev/null
printf 'beta v2\n' > dir/b.txt; $MG add dir/b.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG switch main >/dev/null 2>&1; echo "   switch main exit=$?  dir/b.txt=$(cat dir/b.txt)  git status: $(git -c gc.auto=0 status --porcelain | tr '\n' '|')"
$MG checkout side >/dev/null 2>&1; echo "   checkout side exit=$?  dir/b.txt=$(cat dir/b.txt)  git status: $(git -c gc.auto=0 status --porcelain | tr '\n' '|')"
$MG reset --hard main >/dev/null 2>&1; echo "   reset --hard main exit=$?  dir/b.txt=$(cat dir/b.txt)"

echo
echo "=== clean auto-merge (non-ff) with nested paths"
R=$W/auto_nested; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; mkdir dir; printf 'beta\n' > dir/b.txt; printf 'alpha\n' > a.txt
$MG add . >/dev/null; $MG commit -m c1 >/dev/null
$MG switch -c side >/dev/null; printf 'side file\n' > side.txt; $MG add side.txt >/dev/null; $MG commit -m c2 >/dev/null
$MG switch main >/dev/null; printf 'main file\n' > main.txt; $MG add main.txt >/dev/null; $MG commit -m c3 >/dev/null
$MG merge side > /tmp/t15/matrix/auto.out 2>/tmp/t15/matrix/auto.err
echo "   mg merge exit=$?  stdout=$(cat /tmp/t15/matrix/auto.out | tr '\n' '|')  stderr=$(cat /tmp/t15/matrix/auto.err | tr '\n' '|')"
echo "   git status: $(git -c gc.auto=0 status --porcelain | tr '\n' '|')"
echo "   git log: $(git -c gc.auto=0 log --oneline | tr '\n' '|')"
