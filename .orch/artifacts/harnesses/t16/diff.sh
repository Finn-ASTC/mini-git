#!/usr/bin/env bash
# Differential harness: build identical repos, run `git merge side` vs `mg merge side`, compare fields.
set -u
MG=/home/user/Projects/mini-git/target/debug/mg
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=V GIT_AUTHOR_EMAIL=v@example.com
export GIT_COMMITTER_NAME=V GIT_COMMITTER_EMAIL=v@example.com
export GIT_AUTHOR_DATE="1700000000 +0000" GIT_COMMITTER_DATE="1700000000 +0000"
export GIT_MERGE_AUTOEDIT=no

ROOT=/tmp/t16/diff
rm -rf "$ROOT"; mkdir -p "$ROOT"
FAILS=0

listing() {
  local d="$1"
  ( cd "$d" && find . -path ./.git -prune -o -print | sort | while read -r p; do
      [ "$p" = "." ] && continue
      if [ -L "$p" ]; then printf 'L %s -> %s\n' "$p" "$(readlink "$p")";
      elif [ -d "$p" ]; then printf 'D %s\n' "$p";
      else printf 'F %s %s\n' "$(stat -c %a "$p")" "$p"; fi
    done )
}

compare() {
  local name="$1" gitdir="$2" mgdir="$3"
  local a b
  # Compare tree+subject (merge-commit oid legitimately differs: mg signs with its own
  # committer identity/timestamp, so `--oneline` hashes are not a valid oracle here).
  a=$(cd "$gitdir" && git log --format='%T %s' 2>&1); b=$(cd "$mgdir" && git log --format='%T %s' 2>&1)
  if [ "$a" != "$b" ]; then echo "  [FAIL] $name log"; diff <(printf '%s\n' "$a") <(printf '%s\n' "$b")|sed 's/^/    /'; FAILS=1; else echo "  [ok] $name log(tree+subject)"; fi
  a=$(cd "$gitdir" && git log --format='%P' -1 2>&1); b=$(cd "$mgdir" && git log --format='%P' -1 2>&1)
  if [ "$a" != "$b" ]; then echo "  [FAIL] $name parents"; diff <(printf '%s\n' "$a") <(printf '%s\n' "$b")|sed 's/^/    /'; FAILS=1; else echo "  [ok] $name parents"; fi
  a=$(cd "$gitdir" && git status --porcelain 2>&1); b=$(cd "$mgdir" && git status --porcelain 2>&1)
  if [ "$a" != "$b" ]; then echo "  [FAIL] $name status"; diff <(printf '%s\n' "$a") <(printf '%s\n' "$b")|sed 's/^/    /'; FAILS=1; else echo "  [ok] $name status"; fi
  a=$(cd "$gitdir" && git ls-files --stage 2>&1); b=$(cd "$mgdir" && git ls-files --stage 2>&1)
  if [ "$a" != "$b" ]; then echo "  [FAIL] $name index"; diff <(printf '%s\n' "$a") <(printf '%s\n' "$b")|sed 's/^/    /'; FAILS=1; else echo "  [ok] $name index"; fi
  a=$(listing "$gitdir"); b=$(listing "$mgdir")
  if [ "$a" != "$b" ]; then echo "  [FAIL] $name worktree"; diff <(printf '%s\n' "$a") <(printf '%s\n' "$b")|sed 's/^/    /'; FAILS=1; else echo "  [ok] $name worktree"; fi
  local f1 f2
  # dangling objects are normal leftovers; only real corruption is a failure
  f1=$(cd "$gitdir" && git fsck --no-progress 2>&1 | grep -Ev '^(悬空|dangling) ' || true)
  f2=$(cd "$mgdir" && git fsck --no-progress 2>&1 | grep -Ev '^(悬空|dangling) ' || true)
  if [ -n "$f1" ] || [ -n "$f2" ]; then echo "  [FAIL] $name fsck git='$f1' mg='$f2'"; FAILS=1; else echo "  [ok] $name fsck-clean"; fi
}

run_scenario() {
  local name="$1" setup="$2"
  local base="$ROOT/$name" origin="$ROOT/$name/origin" mgside="$ROOT/$name/mg"
  mkdir -p "$origin"; ( cd "$origin" && git init -q -b main )
  "$setup" "$origin"
  cp -a "$origin" "$mgside"
  echo "== scenario: $name =="
  local gout mout grc mrc
  gout=$(cd "$origin" && git merge side 2>&1); grc=$?
  mout=$(cd "$mgside" && "$MG" merge side 2>&1); mrc=$?
  if [ "$grc" -ne "$mrc" ]; then
    echo "  [FAIL] exit codes git=$grc mg=$mrc"; echo "    git: $gout"; echo "    mg : $mout"; FAILS=1
  else
    echo "  [ok] exit codes ($grc)"
  fi
  # conflict-file bytes (subdir scenarios)
  for f in dir/f.txt dir/b.txt; do
    if [ -f "$origin/$f" ] && [ -f "$mgside/$f" ]; then
      if cmp -s "$origin/$f" "$mgside/$f"; then echo "  [ok] $name bytes $f"; else echo "  [FAIL] $name bytes $f"; FAILS=1; fi
    fi
  done
  compare "$name" "$origin" "$mgside"
}

mk_nested_ff() {
  local o="$1"; ( cd "$o"; mkdir -p dir; echo beta > dir/b.txt; echo keep > dir/keep.txt; echo root > root.txt
    git add -A; git commit -qm base; git checkout -q -b side; echo "beta v2" > dir/b.txt
    git add -A; git commit -qm "side edits b"; git checkout -q main )
}
mk_ff_delete_subdir() {
  local o="$1"; ( cd "$o"; mkdir -p dir; echo a > dir/a.txt; echo b > dir/b.txt
    git add -A; git commit -qm base; git checkout -q -b side; git rm -q dir/b.txt
    git commit -qm "side removes b"; git checkout -q main )
}
mk_ff_add_multilevel() {
  local o="$1"; ( cd "$o"; echo root > root.txt; git add -A; git commit -qm base
    git checkout -q -b side; mkdir -p a/b/c; echo deep > a/b/c/d.txt; echo mid > a/b/e.txt
    git add -A; git commit -qm "side adds deep"; git checkout -q main )
}
mk_ff_symlink_exec() {
  local o="$1"; ( cd "$o"; mkdir -p dir; echo beta > dir/b.txt; git add -A; git commit -qm base
    git checkout -q -b side; ln -s dir/b.txt link; printf '#!/bin/sh\n' > run.sh; chmod +x run.sh
    git add -A; git commit -qm "side adds link+exec"; git checkout -q main )
}
mk_nonff_clean_subdir() {
  local o="$1"; ( cd "$o"; mkdir -p dir; echo base > dir/f.txt; echo base > other.txt
    git add -A; git commit -qm base; git checkout -q -b side; echo theirs > dir/f.txt; echo side > newside.txt
    git add -A; git commit -qm side; git checkout -q main; echo ours > other.txt
    git add -A; git commit -qm main )
}
mk_nonff_conflict_subdir() {
  local o="$1"; ( cd "$o"; mkdir -p dir; printf 'a\nb\nc\nd\ne\n' > dir/f.txt
    git add -A; git commit -qm base; git checkout -q -b side; printf 'a\nTHEIRS\nc\nd\ne\n' > dir/f.txt
    git add -A; git commit -qm side; git checkout -q main; printf 'a\nOURS\nc\nd\ne\n' > dir/f.txt
    git add -A; git commit -qm main )
}
mk_nonff_add_delete_subdir() {
  local o="$1"; ( cd "$o"; mkdir -p dir; echo a > dir/a.txt; echo b > dir/b.txt
    git add -A; git commit -qm base; git checkout -q -b side; echo "b theirs" > dir/b.txt
    git add -A; git commit -qm side; git checkout -q main; git rm -q dir/a.txt
    git add -A; git commit -qm main )
}

run_scenario nested_ff mk_nested_ff
run_scenario ff_delete_subdir mk_ff_delete_subdir
run_scenario ff_add_multilevel mk_ff_add_multilevel
run_scenario ff_symlink_exec mk_ff_symlink_exec
run_scenario nonff_clean_subdir mk_nonff_clean_subdir
run_scenario nonff_conflict_subdir mk_nonff_conflict_subdir
run_scenario nonff_add_delete_subdir mk_nonff_add_delete_subdir

echo
echo "TOTAL_FAILS=$FAILS"
exit $FAILS
