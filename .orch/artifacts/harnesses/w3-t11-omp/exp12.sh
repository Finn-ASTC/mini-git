set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp12; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
d="$ROOT/r7"; mkdir -p "$d"; cd "$d"; $G init -q
printf 'A\n' > a.txt; $G add -A >/dev/null; $G commit -qm one
$G branch side; $G checkout -q -b other; printf 'B\n' > b.txt; $G add -A >/dev/null; $G commit -qm two
$G checkout -q main
$G merge --no-commit --no-ff other >/dev/null 2>&1 || true
echo "MERGE_HEAD exists=$([ -e .git/MERGE_HEAD ] && echo yes || echo no) status=[$($G status --porcelain|tr '\n' '|')]"
echo "--- checkout side (no conflicts, merge in progress):"
$G checkout side 2>&1 | sed 's/^/  /'; echo "  exit=$? HEAD=$(cat .git/HEAD)"
echo "--- switch side:"
$G switch side 2>&1 | sed 's/^/  /'; echo "  exit=$? HEAD=$(cat .git/HEAD)"
echo "--- checkout -f side:"
$G checkout -f side 2>&1 | sed 's/^/  /'; echo "  exit=$? HEAD=$(cat .git/HEAD) MERGE_HEAD=$([ -e .git/MERGE_HEAD ] && echo yes || echo no)"
echo DONE
