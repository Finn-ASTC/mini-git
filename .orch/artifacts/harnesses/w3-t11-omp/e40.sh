set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp6; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
mkconflict() { # creates a UU conflict state in cwd
  printf 'base\n' > c.txt; printf 'k\n' > k.txt; $G add -A >/dev/null; $G commit -qm one
  $G checkout -q -b side; printf 'side\n' > c.txt; $G commit -qam side
  $G checkout -q main; printf 'main\n' > c.txt; $G commit -qam main
  $G merge side >/dev/null 2>&1 || true
}
say "E40a checkout branch with conflicted index"
d=$(newrepo e40a); cd "$d"; mkconflict
echo "  ls-files=[$($G ls-files --stage|tr '\n' '|')]"
$G checkout -q side 2>&1 | sed 's/^/  /'; echo "  exit=$? HEAD=$(cat .git/HEAD)"

say "E40b checkout <rev> -- <path> with conflicted index"
d=$(newrepo e40b); cd "$d"; mkconflict
$G checkout HEAD -- k.txt 2>&1 | sed 's/^/  /'; echo "  exit=$?"
$G checkout side -- c.txt 2>&1 | sed 's/^/  /'; echo "  exit=$? c=$(cat c.txt)"

say "E40c reset --hard with conflicted index"
d=$(newrepo e40c); cd "$d"; mkconflict
$G reset -q --hard HEAD 2>&1 | sed 's/^/  /'; echo "  exit=$? status=[$($G status --porcelain|tr '\n' '|')] ls-files=[$($G ls-files --stage|tr '\n' '|')]"

say "E40d switch with conflicted index"
d=$(newrepo e40d); cd "$d"; mkconflict
$G switch -q side 2>&1 | sed 's/^/  /'; echo "  exit=$? HEAD=$(cat .git/HEAD)"

say "E41 reset --hard on unborn HEAD ignoring rev"
d=$(newrepo e41); cd "$d"; printf 'A\n' > f.txt; $G add -A >/dev/null; $G commit -qm one
$G checkout -q --orphan fresh >/dev/null 2>&1
git -C . rm -q --cached -r . >/dev/null 2>&1; rm -f f.txt
echo "  HEAD=[$(cat .git/HEAD)] worktree=[$(ls -A | grep -v '^\.git$' | tr '\n' ' ')]"
$G rev-parse --verify HEAD 2>&1 | sed 's/^/  /'
echo DONE
