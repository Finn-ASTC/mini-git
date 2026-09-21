#!/usr/bin/env bash
set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp2
rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
say() { printf '\n=== %s ===\n' "$1"; }

say "E14 ORIG_HEAD bytes"
d=$(newrepo e14); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
printf 'A2\n' > f.txt; $G commit -qam two
$G reset -q --soft HEAD~1
echo "  soft size=$(wc -c < .git/ORIG_HEAD) content=[$(cat .git/ORIG_HEAD)]"
od -c .git/ORIG_HEAD | tail -2 | sed 's/^/  /'
echo "  old-head=$($G rev-parse HEAD@{1} 2>/dev/null || true)"

say "E15 reset in repo with unborn HEAD"
d=$(newrepo e15); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null
$G commit -qm one
$G commit -q --allow-empty -m two
$G checkout -q --orphan orphan >/dev/null 2>&1; rm -f f.txt
$G reset -q --hard HEAD 2>&1 | sed 's/^/  /'
echo "  ORIG_HEAD=$([ -e .git/ORIG_HEAD ] && echo present || echo missing)"
d=$(newrepo e15b); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G update-ref -d refs/heads/main 2>/dev/null; rm -f .git/index; rm -f f.txt
echo "  unborn: HEAD=$(cat .git/HEAD) file-exists=$([ -e f.txt ] && echo yes || echo no)"

say "E16 checkout current branch with dirty worktree"
d=$(newrepo e16); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
printf 'DIRTY\n' > f.txt
$G checkout main 2>&1 | sed 's/^/  /'; echo "  exit=$? f=$(cat f.txt) status=[$($G status --porcelain)]"
$G switch main 2>&1 | sed 's/^/  /'; echo "  exit=$? f=$(cat f.txt)"

say "E17 empty dir pruning on checkout"
d=$(newrepo e17); cd "$d"
mkdir -p gone; printf 'X\n' > gone/x.txt; printf 'K\n' > keep.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; $G rm -qr gone; $G commit -qm two
$G checkout -q main
$G checkout other 2>&1|sed 's/^/  /'
echo "  gone-dir-exists=$([ -d gone ] && echo yes || echo no) files=[$(find . -path ./.git -prune -o -type f -print | tr '\n' ' ')]"

say "E18 branch -d unmerged / nonexistent"
d=$(newrepo e18); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b side; printf 'S\n' > s.txt; $G add -A >/dev/null; $G commit -qm side
$G checkout -q main; printf 'M\n' > m.txt; $G add -A >/dev/null; $G commit -qm main
$G branch -d side 2>&1 | sed 's/^/  /'; echo "  exit=$?"
$G branch -d ghost 2>&1 | sed 's/^/  /'; echo "  exit=$?"
$G branch -m side2 2>&1 | sed 's/^/  /'; echo "  exit=$? (rename missing)"
$G branch -m main main 2>&1 | sed 's/^/  /'; echo "  exit=$?"
$G branch existing-check 2>&1 >/dev/null; $G branch main 2>&1 | sed 's/^/  /'; echo "  exit=$?"

say "E19 rename to existing name"
$G branch other-b >/dev/null; $G branch -m other-b main 2>&1 | sed 's/^/  /'; echo "  exit=$?"

say "E20 switch -c with start, and checkout -b (non-goal)"
d=$(newrepo e20); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G branch base >/dev/null; printf 'B\n' > f.txt; $G commit -qam two
$G switch -c feat base 2>&1 | sed 's/^/  /'
echo "  HEAD=$(cat .git/HEAD) f=$(cat f.txt) status=[$($G status --porcelain)]"

say "E21 detached head then checkout branch back: index/status parity"
$G checkout -q main
echo "  HEAD=$(cat .git/HEAD) status=[$($G status --porcelain)]"
$G checkout -q - 2>&1 | sed 's/^/  /'; echo "  HEAD=$(cat .git/HEAD)"

say "E22 symlink in worktree: content bytes and exec modes after checkout"
d=$(newrepo e22); cd "$d"
printf 'target-content\n' > real.txt; ln -s real.txt link.txt; printf '#!/bin/sh\n' > run.sh; chmod 755 run.sh; : > empty.txt
$G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; $G rm -q real.txt link.txt run.sh empty.txt; printf 'z\n' > z.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main
ls -l | sed 's/^/  /'
echo "  ls-files=[$($G ls-files --stage | tr '\n' '|')]"

say "E23 non-utf8 filename checkout"
d=$(newrepo e23); cd "$d"
printf 'A\n' > "$(printf 'bad\xff.txt')"; printf 'B\n' > plain.txt
$G add -A >/dev/null; $G -c core.quotepath=false commit -qm one
$G checkout -q -b other; rm -- "$(printf 'bad\xff.txt')"; printf 'B2\n' > plain.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main 2>&1|sed 's/^/  /'
find . -path ./.git -prune -o -type f -print | od -c | head -5 | sed 's/^/  /'
echo "  ls-files=[$($G ls-files --stage -z | od -c | head -3 | tr '\n' ' ')]"
echo "  status=[$($G status --porcelain)]"
echo DONE
