set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp14; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
d=$(newrepo f1); cd "$d"
mkdir -p sub; printf 'inner\n' > sub/i.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q -b other; $G rm -qr sub; printf 'NOW-FILE\n' > sub; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main
printf 'UNTRACKED\n' > sub/keep.txt
echo "--- git checkout -f other with a non-empty untracked dir blocking an incoming file:"
$G checkout -f other 2>&1 | sed 's/^/  /'; echo "  exit=$? sub=$([ -d sub ] && echo dir || cat sub) keep=$([ -e sub/keep.txt ] && echo present || echo gone)"
echo "--- same, but the untracked dir is ignored:"
cd "$d"; $G checkout -q main 2>/dev/null || true
rm -f sub/keep.txt; rmdir sub 2>/dev/null || true
