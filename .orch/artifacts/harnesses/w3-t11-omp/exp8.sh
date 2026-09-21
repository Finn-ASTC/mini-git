set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp8; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main -c gc.auto=0"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
mkstate() { # MM state on main, br0 == HEAD
  printf 'A\n' > f.txt; printf 'K\n' > k.txt; $G add -A >/dev/null; $G commit -qm one
  $G branch br0
  printf 'B\n' > f.txt; $G add f.txt; printf 'C\n' > f.txt
  cp .git/index /tmp/w3-t11-omp/index-before
}
say() { printf '\n=== %s ===\n' "$1"; }
state() { echo "  status=[$($G status --porcelain|tr '\n' '|')] f=$(cat f.txt) index-same=$(cmp -s /tmp/w3-t11-omp/index-before .git/index && echo yes || echo no)"; }

say "P1 switch br0 (same commit)"
d=$(newrepo p1); cd "$d"; mkstate
$G switch br0 2>&1 | sed 's/^/  /'; state

say "P2 checkout --detach HEAD (same commit)"
d=$(newrepo p2); cd "$d"; mkstate
$G checkout --detach HEAD 2>&1 | sed 's/^/  /'; echo "  HEAD=$(cat .git/HEAD)"; state

say "P3 switch -c brandnew br0 (same commit)"
d=$(newrepo p3); cd "$d"; mkstate
$G switch -c brandnew br0 2>&1 | sed 's/^/  /'; echo "  HEAD=$(cat .git/HEAD)"; state

say "P4 switch -f br0 (same commit, force)"
d=$(newrepo p4); cd "$d"; mkstate
$G switch -f br0 2>&1 | sed 's/^/  /'; state

say "P5 checkout -f --detach HEAD (same commit, force)"
d=$(newrepo p5); cd "$d"; mkstate
$G checkout -f --detach HEAD 2>&1 | sed 's/^/  /'; state

say "P6 switch br0 where br0 is a DIFFERENT commit that does not change f"
d=$(newrepo p6); cd "$d"; mkstate
$G checkout -q -b other-branch main; printf 'K2\n' > k.txt; $G commit -qam "only k"
$G checkout -q main 2>/dev/null || true
printf 'A\n' > f.txt; printf 'B\n' > f.txt; $G add f.txt; printf 'C\n' > f.txt
cp .git/index /tmp/w3-t11-omp/index-before
$G switch other-branch 2>&1 | sed 's/^/  /'; state
echo DONE
