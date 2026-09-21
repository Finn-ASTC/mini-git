set -uo pipefail
ROOT=/tmp/w3-t11-omp/exp10; rm -rf "$ROOT"; mkdir -p "$ROOT"
G="git -c user.name=t -c user.email=t@e -c init.defaultBranch=main"
export HOME="$ROOT/home"; mkdir -p "$HOME"; export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null LC_ALL=C
newrepo() { local d="$ROOT/$1"; mkdir -p "$d"; ( cd "$d" && $G init -q ) >/dev/null 2>&1; echo "$d"; }
base() { printf 'A\n' > f.txt; printf 'K\n' > k.txt; $G add -A >/dev/null; $G commit -qm one; $G branch br0; }
say() { printf '\n=== %s ===\n' "$1"; }
run() { # $1 = label, $2 = setup commands
  local d; d=$(newrepo "$1"); cd "$d"; base; eval "$2"
  echo "  before: status=[$($G status --porcelain|tr '\n' '|')]"
  $G switch br0 2>&1 | sed 's/^/  git: /'
  echo "  after: status=[$($G status --porcelain|tr '\n' '|')] files=[$(ls -A | grep -v '^\.git$' | tr '\n' ' ')]"
}

say "Q1 clean index, dirty worktree (M)"
run q1 'printf "DIRTY\n" > f.txt'

say "Q2 index missing an entry that is in the tree (git rm --cached k.txt)"
run q2 '$G rm -q --cached k.txt'

say "Q3 index emptied (read-tree --empty)"
run q3 '$G read-tree --empty'

say "Q5 index has an EXTRA staged file not in the tree"
run q5 'printf "NEW\n" > extra.txt; $G add extra.txt'

say "Q6 index has an entry for a path that the tree has, but the worktree file is a different TYPE (dir)"
run q6 'rm k.txt; mkdir k.txt; printf "x\n" > k.txt/inner.txt'

say "Q7 same commit, but br0 detached-checkout after staging a deletion"
run q7 '$G rm -q --cached f.txt; rm -f f.txt'
echo DONE
