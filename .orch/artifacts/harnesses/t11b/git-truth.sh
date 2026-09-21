#!/bin/bash
# Ground truth: real git behavior for symlink-ancestor delete/write paths.
set -u
R=/tmp/t11b
G="git -c user.name=t -c user.email=t@e"

show() { # show <label> <dir>
  local label="$1" dir="$2" out="$3"
  echo "--- $label"
  echo "outside: $(cd "$out" && find . | sort | tr '\n' ' ')"
  echo "outside-b-c: $(cat "$out/b/c" 2>/dev/null || echo MISSING)"
  echo "a-ctype: $( [ -L "$dir/a" ] && echo symlink || ( [ -d "$dir/a" ] && echo dir || echo missing) )"
  echo "a-link-target: $(readlink "$dir/a" 2>/dev/null || echo -)"
  echo "worktree: $(cd "$dir" && find . -path ./.git -prune -o -print | sort | tr '\n' ' ')"
  echo "status: $(cd "$dir" && git status --porcelain | tr '\n' '|')"
  echo "lsfiles: $(cd "$dir" && git ls-files --stage | tr '\n' '|')"
  echo "head: $(cd "$dir" && git rev-parse --abbrev-ref HEAD) $(cd "$dir" && git rev-parse HEAD | cut -c1-7)"
}

# ---------------------------------------------------------------- fixture F1
# index has a/b/c ; branch prune removes it.
mk_f1() { # mk_f1 <dir>
  local d="$1"
  rm -rf "$d"; mkdir -p "$d"; cd "$d" || exit 1
  git init -q -b main r; cd r || exit 1
  mkdir -p a/b; printf 'X\n' > a/b/c; printf 'k\n' > keep.txt
  git add -A; $G commit -qm one
  $G checkout -q -b prune; git rm -q a/b/c; $G commit -qm prune
  $G checkout -q main
  git status --porcelain > /dev/null
}

run_f1() { # run_f1 <variant-tag> <outside-content-mode: same|dirty> <cmd...>
  local tag="$1" mode="$2"; shift 2
  local dir="$R/f1-$tag" out="$R/f1-$tag-outside"
  mk_f1 "$dir"
  mkdir -p "$out/b"
  if [ "$mode" = same ]; then printf 'X\n' > "$out/b/c"; else printf 'arbitrary\n' > "$out/b/c"; fi
  rm -rf "$dir/r/a"; ln -s "$out" "$dir/r/a"
  cd "$dir/r" || exit 1
  echo "===== F1 tag=$tag content=$mode cmd: $*"
  "$@" > "$dir/stdout" 2> "$dir/stderr"
  local rc=$?
  echo "exit=$rc"
  echo "stderr: $(tr '\n' '|' < "$dir/stderr")"
  echo "stdout: $(tr '\n' '|' < "$dir/stdout")"
  show "$tag" "$dir/r" "$out"
}

# ---------------------------------------------------------------- fixture F2
# index has a/b/c = v1 ; branch other modifies it to v2 (and adds a/new.txt).
mk_f2() { # mk_f2 <dir> <mode: modify|add>
  local d="$1" mode="$2"
  rm -rf "$d"; mkdir -p "$d"; cd "$d" || exit 1
  git init -q -b main r; cd r || exit 1
  mkdir -p a/b; printf 'v1\n' > a/b/c; printf 'k\n' > keep.txt
  git add -A; $G commit -qm one
  $G checkout -q -b other
  if [ "$mode" = modify ]; then printf 'v2\n' > a/b/c; else printf 'new\n' > a/new.txt; fi
  git add -A; $G commit -qm two
  $G checkout -q main
  git status --porcelain > /dev/null
}

run_f2() { # run_f2 <tag> <mode: modify|add> <inside|outside> <cmd...>
  local tag="$1" mode="$2" target="$3"; shift 3
  local dir="$R/f2-$tag" out="$R/f2-$tag-outside"
  mk_f2 "$dir" "$mode"
  mkdir -p "$out"; printf 'marker\n' > "$out/marker.txt"
  if [ "$mode" = modify ]; then mkdir -p "$out/b"; printf 'v2\n' > "$out/b/c"; fi
  if [ "$target" = inside ]; then
    rm -rf "$dir/r/a"; ln -s real "$dir/r/a"
  else
    rm -rf "$dir/r/a"; ln -s "$out" "$dir/r/a"
  fi
  cd "$dir/r" || exit 1
  echo "===== F2 tag=$tag mode=$mode target=$target cmd: $*"
  "$@" > "$dir/stdout" 2> "$dir/stderr"
  local rc=$?
  echo "exit=$rc"
  echo "stderr: $(tr '\n' '|' < "$dir/stderr")"
  echo "stdout: $(tr '\n' '|' < "$dir/stdout")"
  echo "outside: $(cd "$out" && find . | sort | tr '\n' ' ')"
  echo "worktree: $(cd "$dir/r" && find . -path ./.git -prune -o -print | sort | tr '\n' ' ')"
  echo "a-ctype: $( [ -L "$dir/r/a" ] && echo symlink || ( [ -d "$dir/r/a" ] && echo dir || echo missing) )"
  echo "a-link-target: $(readlink "$dir/r/a" 2>/dev/null || echo -)"
  echo "a-b-c: $(cat "$dir/r/a/b/c" 2>/dev/null || echo MISSING)"
  echo "status: $(cd "$dir/r" && git status --porcelain | tr '\n' '|')"
  echo "lsfiles: $(cd "$dir/r" && git ls-files --stage | tr '\n' '|')"
}

# ---------------------------------------------------------------- fixture F3
# ancestor is a regular file (not symlink): index has a/b/c, worktree `a` is a file.
mk_f3() { local d="$1" mode="$2"; mk_f2 "$d" "$mode"; }
run_f3() { # run_f3 <tag> <mode> <cmd...>
  local tag="$1" mode="$2"; shift 2
  local dir="$R/f3-$tag"
  mk_f3 "$dir" "$mode"
  rm -rf "$dir/r/a"; printf 'blocker\n' > "$dir/r/a"
  cd "$dir/r" || exit 1
  echo "===== F3 tag=$tag mode=$mode cmd: $*"
  "$@" > "$dir/stdout" 2> "$dir/stderr"
  echo "exit=$?"
  echo "stderr: $(tr '\n' '|' < "$dir/stderr")"
  echo "worktree: $(cd "$dir/r" && find . -path ./.git -prune -o -print | sort | tr '\n' ' ')"
  echo "a-ctype: $( [ -L "$dir/r/a" ] && echo symlink || ( [ -d "$dir/r/a" ] && echo dir || echo file) )"
  echo "a-content: $(cat "$dir/r/a" 2>/dev/null || echo -)"
  echo "a-b-c: $(cat "$dir/r/a/b/c" 2>/dev/null || echo MISSING)"
  echo "status: $(cd "$dir/r" && git status --porcelain | tr '\n' '|')"
  echo "lsfiles: $(cd "$dir/r" && git ls-files --stage | tr '\n' '|')"
}

echo "############ F1 deletion, symlink -> outside, identical content"
run_f1 same-switch    same  git switch prune
run_f1 same-switchf   same  git switch -f prune
run_f1 same-cof       same  git checkout -f prune
run_f1 same-reset     same  git reset --hard prune
echo "############ F1 deletion, symlink -> outside, dirty content"
run_f1 dirty-switch   dirty git switch prune
run_f1 dirty-switchf  dirty git switch -f prune
run_f1 dirty-reset    dirty git reset --hard prune

echo "############ F2 write, symlink -> outside"
run_f2 mod-switch-modify  modify outside git switch other
run_f2 mod-cof-modify     modify outside git checkout -f other
run_f2 mod-add-newfile    add    outside git switch other
echo "############ F2 write, symlink -> inside worktree"
run_f2 inside-modify      modify inside  git switch other
run_f2 inside-add         add    inside  git switch other

echo "############ F3 ancestor is a regular file"
run_f3 f3-del   modify git switch other
run_f3 f3-del-f modify git switch -f other
