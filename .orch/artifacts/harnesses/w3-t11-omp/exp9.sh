set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp9; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
say() { printf '\n=== %s ===\n' "$1"; }

say "P7 same-commit switch with a MISSING index"
d=$(newrepo p7); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one; $G branch br0
rm .git/index
$G switch br0 2>&1 | sed 's/^/  /'
echo "  index-exists=$([ -e .git/index ] && echo yes || echo no) ls-files=[$($G ls-files --stage|tr '\n' '|')] status=[$($G status --porcelain|tr '\n' '|')]"

say "P8 same-commit switch with the worktree file deleted (index intact)"
d=$(newrepo p8); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one; $G branch br0
rm f.txt
$G switch br0 2>&1 | sed 's/^/  /'
echo "  f=$([ -e f.txt ] && echo present || echo MISSING) status=[$($G status --porcelain|tr '\n' '|')]"

say "P9 same-commit switch where a file is left in the worktree but not in the index (untracked in the way)"
d=$(newrepo p9); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one; $G branch br0
printf 'X\n' > extra.txt
$G switch br0 2>&1 | sed 's/^/  /'
echo "  extra=$([ -e extra.txt ] && echo present || echo MISSING)"

say "P10 detached -> same commit but branch checkout with staged-only change (index==HEAD? no: staged differs)"
d=$(newrepo p10); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one; $G branch br0; $G checkout -q --detach HEAD
printf 'B\n' > f.txt; $G add f.txt
$G checkout br0 2>&1 | sed 's/^/  /'
echo "  HEAD=[$(cat .git/HEAD)] status=[$($G status --porcelain|tr '\n' '|')]"
echo DONE
