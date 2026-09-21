set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp13; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
d="$ROOT/r8"; mkdir -p "$d"; cd "$d"; $G init -q
printf 'base\n' > c.txt; $G add -A >/dev/null; $G commit -qm one
$G branch at-head
$G checkout -q -b side; printf 'side\n' > c.txt; $G commit -qam side
$G checkout -q main; printf 'main\n' > c.txt; $G commit -qam main
$G merge side >/dev/null 2>&1 || true
echo "conflicts=[$($G ls-files --stage | tr '\n' '|')]"
echo "--- checkout at-head (same commit as HEAD, conflicted index):"
$G checkout at-head 2>&1 | sed 's/^/  /'; echo "  exit=$? HEAD=$(cat .git/HEAD) stage=[$($G ls-files --stage|tr '\n' '|')]"
echo "--- switch at-head (from main, conflicted):"
cd "$d"; $G checkout -q main 2>/dev/null; git -C . checkout -q main 2>/dev/null
$G switch at-head 2>&1 | sed 's/^/  /'; echo "  exit=$? HEAD=$(cat .git/HEAD)"
echo "--- checkout -f at-head:"
$G checkout -f at-head 2>&1 | sed 's/^/  /'; echo "  exit=$? status=[$($G status --porcelain|tr '\n' '|')]"
echo DONE
