#!/usr/bin/env bash
# Real-git semantics probes for T11 (checkout/switch/reset edge cases).
set -uo pipefail

ROOT=/tmp/w3-t11-omp/exp
rm -rf "$ROOT"; mkdir -p "$ROOT"

G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C

newrepo() { # $1 = name
  local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"
}

say() { printf '\n=== %s ===\n' "$1"; }

################################################################ E1/E2/E3
d=$(newrepo e1)
cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; printf 'B\n' > f.txt; $G commit -qam two
$G checkout -q main
printf 'B\n' > f.txt                     # local change == target content
say "E1 checkout other with local content equal to target"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  exit=$?  f=$(cat f.txt)  status=[$($G status --porcelain)]"

################################################################ E2
d=$(newrepo e2)
cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; $G commit -q --allow-empty -m two
$G checkout -q main
printf 'A2\n' > f.txt
say "E2 checkout other, f unchanged between branches, dirty locally"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  f=$(cat f.txt) status=[$($G status --porcelain)]"

################################################################ E3
d=$(newrepo e3)
cd "$d"
printf 'A\n' > f.txt; printf 'K\n' > k.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; $G commit -q --allow-empty -m two
$G checkout -q main
rm f.txt
say "E3 checkout other, f deleted in worktree, unchanged between branches"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  exists=$([ -e f.txt ] && echo yes || echo no) status=[$($G status --porcelain)]"

################################################################ E4
d=$(newrepo e4)
cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; printf 'NEW\n' > g.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main
printf 'LOCAL\n' > g.txt   # untracked, differs from target
say "E4 checkout other with untracked file in the way (different content)"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  exit=$? branch=$($G rev-parse --abbrev-ref HEAD) g=$(cat g.txt 2>/dev/null)"
$G checkout -q main 2>/dev/null
printf 'NEW\n' > g.txt     # untracked, identical to target
say "E4b same, but untracked content identical to target"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  branch=$($G rev-parse --abbrev-ref HEAD) g=$(cat g.txt)"

################################################################ E5
d=$(newrepo e5)
cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; $G rm -q f.txt; $G commit -qm two
$G checkout -q main
printf 'MOD\n' > f.txt
say "E5 checkout other where target deletes locally modified f"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  branch=$($G rev-parse --abbrev-ref HEAD) exists=$([ -e f.txt ] && echo yes || echo no) f=$(cat f.txt 2>/dev/null)"

################################################################ E6 path checkout
d=$(newrepo e6)
cd "$d"
printf 'A\n' > f.txt; printf 'B\n' > g.txt; mkdir -p dir; printf 'D\n' > dir/d.txt
$G add -A >/dev/null; $G commit -qm one
printf 'MOD\n' > f.txt; printf 'MOD2\n' > dir/d.txt; printf 'X\n' > dir/untracked.txt
say "E6a git checkout HEAD -- f.txt (locally modified)"
$G checkout HEAD -- f.txt 2>&1 | sed 's/^/  /'
echo "  f=$(cat f.txt) status=[$($G status --porcelain | tr '\n' '|')]"
say "E6b git checkout HEAD -- dir (dir contains local mod + untracked)"
$G checkout HEAD -- dir 2>&1 | sed 's/^/  /'
echo "  dir/d=$(cat dir/d.txt) untracked=$([ -e dir/untracked.txt ] && echo kept || echo gone)"
say "E6c git checkout HEAD -- nonexistent"
$G checkout HEAD -- nope.txt 2>&1 | sed 's/^/  /'
echo "  exit=$?"
say "E6d git checkout HEAD -- <sha-form rev of HEAD>"
$G checkout "$($G rev-parse HEAD)" -- f.txt 2>&1 | sed 's/^/  /'
echo "  exit=$? HEAD=$($G rev-parse --abbrev-ref HEAD) status=[$($G status --porcelain|tr '\n' '|')]"

################################################################ E7 reset
d=$(newrepo e7)
cd "$d"
printf 'A\n' > f.txt; printf 'B\n' > g.txt; $G add -A >/dev/null; $G commit -qm one
printf 'A2\n' > f.txt; $G add -A >/dev/null; $G commit -qm two
for mode in --soft --mixed --hard; do
  say "E7 reset $mode with local modification"
  printf 'DIRTY\n' > g.txt
  $G reset -q "$mode" HEAD~1 2>&1 | sed 's/^/  /'
  echo "  ORIG_HEAD=$([ -e .git/ORIG_HEAD ] && od -c .git/ORIG_HEAD | head -2 | tr '\n' ' ' || echo MISSING)"
  echo "  f=$(cat f.txt) g=$(cat g.txt) status=[$($G status --porcelain | tr '\n' '|')]"
  echo "  ls-files=[$($G ls-files --stage | tr '\n' '|')]"
  echo "  index-debug(g)=[$($G ls-files --debug g.txt 2>/dev/null | tr '\n' '|')]"
  echo "  index-debug(f)=[$($G ls-files --debug f.txt 2>/dev/null | tr '\n' '|')]"
  echo "  HEAD-file=[$(cat .git/HEAD)]"
  # restore for next iteration
  $G reset -q --hard HEAD >/dev/null 2>&1
  printf 'A2\n' > f.txt; printf 'B\n' > g.txt
  $G checkout -q -- . 2>/dev/null
  $G status --porcelain >/dev/null
  $G reset -q --hard "$($G rev-list --max-parents=0 HEAD | head -1)" 2>/dev/null
  $G reset -q --hard HEAD 2>/dev/null
  printf 'A2\n' > f.txt; $G add -A >/dev/null; $G commit -qm two2 >/dev/null 2>&1
done

################################################################ E8 ignored file in the way
d=$(newrepo e8)
cd "$d"
printf 'ignored/\n' > .gitignore; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; mkdir -p ignored; printf 'FROM-BRANCH\n' > ignored/x.txt
$G add -f ignored/x.txt >/dev/null; $G commit -qm two
$G checkout -q main
printf 'LOCAL-IGNORED\n' > ignored/x.txt
say "E8 checkout other where target has a path that is ignored in worktree"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  branch=$($G rev-parse --abbrev-ref HEAD) x=$(cat ignored/x.txt 2>/dev/null)"

################################################################ E9 branch commands
d=$(newrepo e9)
cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G branch feat >/dev/null
$G branch -m feat feature
say "E9a after -m feat feature"; echo "  HEAD=$(cat .git/HEAD)"; $G branch | sed 's/^/  /'
$G checkout -q feature; $G branch -m renamed
say "E9b -m renamed (current)"; echo "  HEAD=$(cat .git/HEAD)"; $G branch | sed 's/^/  /'; echo "  refs=$(ls .git/refs/heads)"
say "E9c branch -d unmerged / merged"
$G checkout -q main; printf 'X\n' > x.txt; $G add -A >/dev/null; $G commit -qm main2
$G branch -d renamed 2>&1 | sed 's/^/  /'; echo "  exit=$?"
$G branch -d main 2>&1 | sed 's/^/  /'; echo "  exit=$?"
say "E9d branch --list --format"; $G branch --list --format='%(refname:short)' | sed 's/^/  /'
say "E9e unborn HEAD: git branch new"
d2=$(newrepo e9b); cd "$d2"; $G branch new 2>&1 | sed 's/^/  /'; echo "  exit=$?"; echo "  HEAD=$(cat .git/HEAD)"
say "E9f checkout of a tag / sha -> detached?"
d3=$(newrepo e9c); cd "$d3"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one; $G tag v1
$G checkout -q v1 2>&1|sed 's/^/  /'; echo "  HEAD=$(cat .git/HEAD)"; echo "  branchout=[$($G branch | tr '\n' '|')]"

################################################################ E11 untracked file where target has a directory
d=$(newrepo e11)
cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; mkdir -p sub; printf 'S\n' > sub/s.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main
printf 'BLOCKER\n' > sub
say "E11 checkout other where untracked FILE blocks an incoming directory"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  branch=$($G rev-parse --abbrev-ref HEAD)"

################################################################ E12 tracked file replaced by dir / dir replaced by file
d=$(newrepo e12)
cd "$d"
mkdir -p a; printf 'INNER\n' > a/b.txt; printf 'F\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; $G rm -qr a; printf 'NOW-A-FILE\n' > a; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main
say "E12 checkout other (a/ -> file a), plus untracked file inside a/"
printf 'UNTRACKED\n' > a/keep.txt
$G checkout other 2>&1 | sed 's/^/  /'
echo "  exit=$? branch=$($G rev-parse --abbrev-ref HEAD) a-is-dir=$([ -d a ] && echo yes || echo no)"
rm -f a/keep.txt; rmdir a 2>/dev/null; $G checkout -q other 2>/dev/null || true
$G checkout -q main 2>/dev/null || true
say "E12b same but with empty dir a/"
rm -f a/keep.txt 2>/dev/null; rmdir a 2>/dev/null
$G checkout -q other 2>&1 | sed 's/^/  /'
echo "  branch=$($G rev-parse --abbrev-ref HEAD) a=[$(cat a 2>/dev/null)]"

################################################################ E13 symlink ancestor escape
d=$(newrepo e13)
cd "$d"
mkdir -p real; printf 'R\n' > real/r.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; mkdir -p a; printf 'C\n' > a/c.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main
OUT=/tmp/w3-t11-omp/outside; rm -rf "$OUT"; mkdir -p "$OUT"
ln -s "$OUT" a     # untracked symlink to outside
say "E13 checkout other with untracked symlink 'a' -> outside, target has a/c.txt"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  branch=$($G rev-parse --abbrev-ref HEAD) outside=[$(ls -A "$OUT" | tr '\n' ' ')] symlink=$([ -L a ] && echo yes || echo no)"
$G checkout -q main 2>/dev/null || true
say "E13b same but 'a' is a TRACKED symlink (main has symlink a)"
d4=$(newrepo e13b); cd "$d4"
OUT2=/tmp/w3-t11-omp/outside2; rm -rf "$OUT2"; mkdir -p "$OUT2"
ln -s "$OUT2" a; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; rm a; mkdir -p a; printf 'C\n' > a/c.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main; echo "  at main a-is-symlink=$([ -L a ] && echo yes || echo no)"
$G checkout other 2>&1 | sed 's/^/  /'
echo "  branch=$($G rev-parse --abbrev-ref HEAD) a-is-dir=$([ -d a ] && echo yes || echo no) outside2=[$(ls -A "$OUT2"|tr '\n' ' ')]"
echo DONE
