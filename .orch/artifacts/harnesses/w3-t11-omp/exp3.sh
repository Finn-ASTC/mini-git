#!/usr/bin/env bash
set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp3
rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
say() { printf '\n=== %s ===\n' "$1"; }

say "E24 checkout -f on the SAME branch with dirty worktree"
d=$(newrepo e24); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
printf 'DIRTY\n' > f.txt
$G checkout -f main 2>&1 | sed 's/^/  /'
echo "  exit=$? f=$(cat f.txt) status=[$($G status --porcelain)]"
printf 'DIRTY\n' > f.txt
$G switch -f main 2>&1 | sed 's/^/  /'
echo "  exit=$? f=$(cat f.txt)"

say "E25 checkout -f with untracked file in the way"
d=$(newrepo e25); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; printf 'NEW\n' > g.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main; printf 'LOCAL\n' > g.txt
$G checkout -f other 2>&1 | sed 's/^/  /'
echo "  exit=$? branch=$($G rev-parse --abbrev-ref HEAD) g=$(cat g.txt)"

say "E26 switch -c new with dirty conflicting worktree"
d=$(newrepo e26); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; printf 'B\n' > f.txt; $G commit -qam two
$G checkout -q main; printf 'DIRTY\n' > f.txt
$G switch -c from-other other 2>&1 | sed 's/^/  /'; echo "  exit=$? HEAD=$(cat .git/HEAD) f=$(cat f.txt)"
echo "  branches=[$($G branch --list --format='%(refname:short)' | tr '\n' '|')]"

say "E27 switch -c new with dirty NON-conflicting worktree"
$G checkout -q main 2>/dev/null; printf 'DIRTY\n' > f.txt
$G switch -c fresh main 2>&1 | sed 's/^/  /'; echo "  HEAD=$(cat .git/HEAD) f=$(cat f.txt) status=[$($G status --porcelain)]"

say "E28 checkout <old-rev> -- <dir> where dir gained a file after rev"
d=$(newrepo e28); cd "$d"
mkdir -p dir; printf 'D\n' > dir/d.txt; $G add -A >/dev/null; $G commit -qm one
printf 'D2\n' > dir/d.txt; printf 'NEW\n' > dir/new.txt; $G add -A >/dev/null; $G commit -qm two
printf 'D3\n' > dir/d.txt; printf 'NEW3\n' > dir/new.txt
$G checkout HEAD~1 -- dir 2>&1 | sed 's/^/  /'
echo "  exit=$? d=$(cat dir/d.txt) new=$([ -e dir/new.txt ] && cat dir/new.txt || echo GONE)"
echo "  status=[$($G status --porcelain|tr '\n' '|')] ls-files=[$($G ls-files --stage|tr '\n' '|')]"

say "E29 reset --hard with untracked + staged changes and ORIG_HEAD in detached head"
d=$(newrepo e29); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
printf 'B\n' > g.txt; $G add -A >/dev/null; $G commit -qm two
printf 'U\n' > untracked.txt
$G checkout -q --detach HEAD~1
printf 'S\n' > f.txt; $G add -A >/dev/null
$G reset -q --hard HEAD 2>&1 | sed 's/^/  /'
echo "  HEAD-file=[$(cat .git/HEAD)] f=$(cat f.txt) untracked=$([ -e untracked.txt ] && echo kept || echo gone) status=[$($G status --porcelain|tr '\n' '|')]"
echo "  ORIG_HEAD=$([ -e .git/ORIG_HEAD ] && cat .git/ORIG_HEAD || echo missing)"

say "E30 reset --hard <branch> while attached: ref + HEAD + index"
d=$(newrepo e30); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b side; printf 'S\n' > side.txt; $G add -A >/dev/null; $G commit -qm side
$G checkout -q main
$G reset -q --hard side 2>&1 | sed 's/^/  /'
echo "  HEAD=$(cat .git/HEAD) refs/heads/main=$($G rev-parse main) side=$($G rev-parse side) status=[$($G status --porcelain)]"
echo DONE
