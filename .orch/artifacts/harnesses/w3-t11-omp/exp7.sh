set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp7; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
say() { printf '\n=== %s ===\n' "$1"; }

say "X1: MM (staged B, worktree C), checkout --detach <same commit>"
d=$(newrepo x1); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
printf 'B\n' > f.txt; $G add f.txt; printf 'C\n' > f.txt
echo "  status=[$($G status --porcelain|tr '\n' '|')]"
$G checkout --detach HEAD 2>&1 | sed 's/^/  /'; echo "  exit=$? f=$(cat f.txt) status=[$($G status --porcelain|tr '\n' '|')]"

say "X2: MM, checkout another branch that changes f.txt"
d=$(newrepo x2); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G branch other; printf 'B\n' > f.txt; $G add f.txt; printf 'C\n' > f.txt
git -C . checkout -q main 2>/dev/null
echo "  status=[$($G status --porcelain|tr '\n' '|')]"
$G checkout other 2>&1 | sed 's/^/  /'; echo "  exit=$? f=$(cat f.txt)"

say "X3: M only (staged B, worktree = B, i.e. clean worktree vs index)"
d=$(newrepo x3); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G branch other; printf 'B\n' > f.txt; $G add f.txt
echo "  status=[$($G status --porcelain|tr '\n' '|')]"
$G checkout other 2>&1 | sed 's/^/  /'; echo "  exit=$? f=$(cat f.txt)"

say "X4: worktree deletion of a STAGED-added file (A staged, then deleted from worktree)"
d=$(newrepo x4); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G branch other; printf 'B\n' > f.txt; $G add f.txt; rm f.txt
echo "  status=[$($G status --porcelain|tr '\n' '|')]"
$G checkout other 2>&1 | sed 's/^/  /'; echo "  exit=$? f=$([ -e f.txt ] && cat f.txt || echo MISSING)"

say "X5: conflict-state-ish: MM where worktree content == target content"
d=$(newrepo x5); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G branch other; $G checkout -q other; printf 'T\n' > f.txt; $G commit -qam other; $G checkout -q main
printf 'B\n' > f.txt; $G add f.txt; printf 'T\n' > f.txt
echo "  status=[$($G status --porcelain|tr '\n' '|')]"
$G checkout other 2>&1 | sed 's/^/  /'; echo "  exit=$? f=$(cat f.txt)"
echo DONE
