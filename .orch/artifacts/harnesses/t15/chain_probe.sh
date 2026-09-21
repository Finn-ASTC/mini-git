#!/usr/bin/env bash
# Probe the whole CLI chain: mg operation then compare with real git on the SAME repo.
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c init.defaultBranch=main -c user.name=A -c user.email=a@example.com -c gc.auto=0 -c commit.gpgsign=false -c protocol.file.allow=always"
W=/tmp/t15/chain; rm -rf "$W"; mkdir -p "$W"; cd "$W"

cmp2() { # cmp2 <label> <cmd...>  -> runs mg and git with same args, compares stdout
  local label="$1"; shift
  local mo go mrc grc
  mo=$($MG "$@" 2>/tmp/t15/chain/mg.err); mrc=$?
  go=$($G "$@" 2>/tmp/t15/chain/git.err); grc=$?
  echo "--- $label [$*]  mg exit=$mrc git exit=$grc"
  if [ "$mo" = "$go" ]; then echo "    stdout IDENTICAL:"; printf '%s\n' "$mo" | sed 's/^/      | /'
  else echo "    stdout DIFFERS:"; echo "      mg:"; printf '%s\n' "$mo" | sed 's/^/      | /'; echo "      git:"; printf '%s\n' "$go" | sed 's/^/      | /'; fi
}

$MG init . >/dev/null
mkdir dir
printf 'alpha\n' > a.txt; printf 'beta\n' > dir/b.txt; printf 'gamma\n' > c.txt

echo "##### step: hash-object -w"
echo "mg:  $($MG hash-object -w a.txt)"
echo "git: $($G hash-object -w a.txt)"

echo "##### step: add ."
cmp2 "status before add" status --porcelain
$MG add . && echo "mg add ok"
cmp2 "status after add" status --porcelain
echo "git ls-files --stage:"; $G ls-files --stage

echo "##### step: diff --staged"
cmp2 "diff --staged" diff --staged

echo "##### step: commit"
$MG commit -m 'first' >/dev/null; echo "mg commit rc=$?"
cmp2 "log --oneline" log --oneline
cmp2 "status --porcelain (clean)" status --porcelain
echo "git write-tree == HEAD^{tree}? $($G write-tree) vs $($G rev-parse 'HEAD^{tree}')"
echo "git fsck: $($G fsck --no-progress 2>&1; echo rc=$?)"

echo "##### step: worktree modification + mg diff"
printf 'alpha v2\n' > a.txt; printf 'delta\n' > d.txt
cmp2 "diff (worktree, unstaged changes + untracked)" diff
cmp2 "status --porcelain (modified+untracked)" status --porcelain
$MG add a.txt
cmp2 "diff --staged after add a.txt" diff --staged
cmp2 "status --porcelain (mixed)" status --porcelain
$MG commit -m 'second' >/dev/null
cmp2 "log --oneline (after 2nd)" log --oneline

echo "##### step: tags"
$MG tag v1 >/dev/null && echo "mg tag v1 ok"
$MG tag -a va -m 'annotated tag' >/dev/null && echo "mg tag -a va ok"
cmp2 "show-ref --tags" show-ref --tags
echo "git cat-file -t va: $($G cat-file -t va)"
echo "git rev-parse v1^{commit} vs HEAD: $($G rev-parse 'v1^{commit}') / $($G rev-parse HEAD)"
cmp2 "tag --list" tag --list

echo "##### step: branch + switch"
$MG branch feature >/dev/null && echo "mg branch feature ok"
cmp2 "branch --list" branch --list
$MG switch feature >/dev/null && echo "mg switch feature ok"
echo "git symbolic-ref HEAD: $($G symbolic-ref HEAD)"
cmp2 "status --porcelain (on feature)" status --porcelain

printf 'feature change\n' > f.txt
$MG add f.txt >/dev/null && $MG commit -m 'feature work' >/dev/null
cmp2 "log --oneline (feature)" log --oneline
$MG switch main >/dev/null && echo "mg switch main ok"
echo "git symbolic-ref HEAD: $($G symbolic-ref HEAD)"

echo "##### step: merge feature (fast-forward or merge)"
$MG merge feature; echo "mg merge rc=$?"
cmp2 "log --oneline (after merge)" log --oneline
cmp2 "status --porcelain (after merge)" status --porcelain
echo "git rev-list --parents -1 HEAD: $($G rev-list --parents -1 HEAD)"

echo "##### step: merge conflict"
$MG branch other >/dev/null
$MG switch other >/dev/null
printf 'other version\n' > a.txt
$MG add a.txt >/dev/null; $MG commit -m 'other edits a.txt' >/dev/null
$MG switch main >/dev/null
printf 'main version\n' > a.txt
$MG add a.txt >/dev/null; $MG commit -m 'main edits a.txt' >/dev/null
echo "-- mg merge other (expect conflict):"
$MG merge other; echo "   mg merge rc=$?"
echo "-- git status --porcelain:"; $G status --porcelain
echo "-- mg status --porcelain:"; $MG status --porcelain
echo "-- git ls-files -u:"; $G ls-files -u
echo "-- a.txt content:"; cat a.txt
echo "-- git diff (conflict):"; $G diff | head -20
echo "-- mg diff:"; $MG diff 2>&1 | head -20
echo "-- resolve + commit:"
printf 'resolved\n' > a.txt
$MG add a.txt >/dev/null; $MG commit -m 'merge other' >/dev/null; echo "   mg commit rc=$?"
cmp2 "log --oneline (after merge commit)" log --oneline
echo "git rev-list --parents -1 HEAD: $($G rev-list --parents -1 HEAD)"
echo "git fsck after merge: $($G fsck --no-progress 2>&1; echo rc=$?)"

echo "##### step: reset"
cmp2 "rev-parse HEAD" rev-parse HEAD
echo "-- mg reset --soft HEAD~1"; $MG reset --soft HEAD~1; echo "   rc=$?"
cmp2 "status --porcelain after reset --soft" status --porcelain
cmp2 "diff --staged after reset --soft" diff --staged
echo "-- mg reset --hard HEAD"; $MG reset --hard HEAD; echo "   rc=$?"
cmp2 "status --porcelain after reset --hard" status --porcelain

echo "##### step: checkout"
echo "-- mg checkout main~1 -- a.txt"; $MG checkout 'main~1' -- a.txt; echo "   rc=$?"
echo "-- a.txt: $(cat a.txt)"
echo "-- git status: $($G status --porcelain)"
$MG checkout -- a.txt 2>&1; echo "   checkout -- a.txt rc=$?"
echo "-- mg checkout <branch>:"; $MG checkout main; echo "   rc=$?"
cmp2 "log --oneline (final)" log --oneline

echo "##### step: fsck + gc at the end"
$MG fsck; echo "   mg fsck rc=$?"
$MG gc; echo "   mg gc rc=$?"
echo "-- git fsck: $($G fsck --no-progress 2>&1; echo rc=$?)"
cmp2 "log --oneline after gc" log --oneline
echo "-- count-objects: $($G count-objects -v | tr '\n' ' ')"
