set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp4; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
say() { printf '\n=== %s ===\n' "$1"; }

say "E31 ignored FILE where target needs a directory"
d=$(newrepo e31); cd "$d"
printf 'sub/\n' > .gitignore; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; mkdir -p sub; printf 'FROMBRANCH\n' > sub/s.txt; $G add -f sub/s.txt >/dev/null; $G commit -qm two
$G checkout -q main
printf 'LOCALIGNORED\n' > sub    # ignored file blocking an incoming dir
$G checkout other 2>&1 | sed 's/^/  /'
echo "  branch=$($G rev-parse --abbrev-ref HEAD) sub-is-dir=$([ -d sub ] && echo yes || echo no) sub=[$(cat sub 2>/dev/null)]"

say "E32 tracked file -> directory where worktree file is dirty"
d=$(newrepo e32); cd "$d"
printf 'A\n' > a; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; rm a; mkdir -p a; printf 'INNER\n' > a/x.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main
printf 'DIRTYA\n' > a
$G checkout other 2>&1 | sed 's/^/  /'
echo "  branch=$($G rev-parse --abbrev-ref HEAD) a-is-dir=$([ -d a ] && echo yes || echo no)"

say "E33 checkout <rev> -- <missing path> exit code, and <path> not in rev but tracked"
d=$(newrepo e33); cd "$d"; printf 'A\n' > a.txt; $G add -A >/dev/null; $G commit -qm one
printf 'B\n' > b.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q HEAD~1 -- b.txt 2>&1 | sed 's/^/  /'; echo "  exit=$? b=$([ -e b.txt ] && cat b.txt || echo GONE)"
$G checkout -q HEAD~1 -- a.txt 2>&1 | sed 's/^/  /'; echo "  exit=$? a=$(cat a.txt) status=[$($G status --porcelain|tr '\n' '|')]"

say "E34 reset --soft moves branch ref; check index untouched bytes"
d=$(newrepo e34); cd "$d"; printf 'A\n' > a.txt; $G add -A >/dev/null; $G commit -qm one
printf 'A2\n' > a.txt; $G add -A >/dev/null; $G commit -qm two
cp .git/index /tmp/w3-t11-omp/index-before
$G reset -q --soft HEAD~1
cmp /tmp/w3-t11-omp/index-before .git/index && echo "  index identical after --soft" || echo "  index CHANGED after --soft"
echo "  ls-files=[$($G ls-files --stage|tr '\n' '|')] status=[$($G status --porcelain|tr '\n' '|')]"

say "E35 detached checkout of a branch name with --detach"
d=$(newrepo e35); cd "$d"; printf 'A\n' > a.txt; $G add -A >/dev/null; $G commit -qm one; $G branch side
$G checkout -q --detach side 2>&1|sed 's/^/  /'; echo "  HEAD=[$(cat .git/HEAD)]"
echo DONE
