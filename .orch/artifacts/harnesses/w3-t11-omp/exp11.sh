set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp11; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
mkconflict() {
  printf 'base\n' > c.txt; printf 'k\n' > k.txt; $G add -A >/dev/null; $G commit -qm one
  $G branch same-commit-branch
  $G checkout -q -b side; printf 'side\n' > c.txt; $G commit -qam side
  $G checkout -q main; printf 'main\n' > c.txt; $G commit -qam main
  $G merge side >/dev/null 2>&1 || true
}
say() { printf '\n=== %s ===\n' "$1"; }

say "R5 conflicted index + switch to a branch at a DIFFERENT commit"
d=$(newrepo r5); cd "$d"; mkconflict
echo "  same-commit-branch points at: $($G rev-parse same-commit-branch | cut -c1-7) (HEAD=$(cat .git/HEAD))"
$G switch same-commit-branch 2>&1 | sed 's/^/  /'; echo "  exit=$? HEAD=$(cat .git/HEAD)"

say "R6 conflicted index + checkout -f <different branch>"
d=$(newrepo r6); cd "$d"; mkconflict
$G checkout -f side 2>&1 | sed 's/^/  /'; echo "  exit=$? status=[$($G status --porcelain|tr '\n' '|')] c=$(cat c.txt)"
echo DONE
