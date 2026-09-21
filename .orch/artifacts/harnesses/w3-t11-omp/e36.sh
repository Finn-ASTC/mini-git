set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp5; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
say() { printf '\n=== %s ===\n' "$1"; }

say "E36 tracked f deleted in worktree; target CHANGES f"
d=$(newrepo e36); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; printf 'B\n' > f.txt; $G commit -qam two; $G checkout -q main
rm f.txt
$G checkout other 2>&1 | sed 's/^/  /'; echo "  exit=$? branch=$($G rev-parse --abbrev-ref HEAD) f=$([ -e f.txt ] && cat f.txt || echo MISSING)"

say "E37 tracked f deleted in worktree; target ALSO deletes f"
d=$(newrepo e37); cd "$d"
printf 'A\n' > f.txt; printf 'K\n' > k.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; $G rm -q f.txt; $G commit -qm two; $G checkout -q main
rm f.txt
$G checkout other 2>&1 | sed 's/^/  /'; echo "  exit=$? branch=$($G rev-parse --abbrev-ref HEAD) f=$([ -e f.txt ] && echo yes || echo no) status=[$($G status --porcelain|tr '\n' '|')]"

say "E38 exec bit flipped locally; target changes content"
d=$(newrepo e38); cd "$d"
printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; printf 'B\n' > f.txt; $G commit -qam two; $G checkout -q main
chmod 755 f.txt
$G checkout other 2>&1 | sed 's/^/  /'; echo "  exit=$? branch=$($G rev-parse --abbrev-ref HEAD)"
$G checkout -q -f main 2>/dev/null; chmod 755 f.txt
say "E38b exec bit flipped locally; f unchanged between branches"
$G checkout other 2>&1 | sed 's/^/  /'; echo "  exit=$? mode=$(stat -c %a f.txt) status=[$($G status --porcelain|tr '\n' '|')]"

say "E39 target adds a file whose path equals an ignored DIRECTORY containing files"
d=$(newrepo e39); cd "$d"
printf 'ign/\n' > .gitignore; printf 'K\n' > k.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; printf 'INCOMING\n' > ign; $G add -f ign >/dev/null; $G commit -qm two; $G checkout -q main
mkdir -p ign; printf 'x\n' > ign/x.txt
$G checkout other 2>&1 | sed 's/^/  /'; echo "  exit=$? branch=$($G rev-parse --abbrev-ref HEAD) ign=[$([ -d ign ] && echo dir || cat ign)]"
echo DONE
