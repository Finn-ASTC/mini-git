#!/bin/bash
# Matrix: git vs current mg for symlink/file ancestors on the delete & write paths.
set -u
R=/tmp/t11b/mx
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c user.name=t -c user.email=t@e"
rm -rf "$R"; mkdir -p "$R"

# fixture: <dir> <kind: del|mod|add>
mkfix() {
  local d="$1" kind="$2"
  rm -rf "$d"; mkdir -p "$d"; cd "$d" || exit 1
  git init -q -b main r; cd r || exit 1
  mkdir -p a/b; printf 'v1\n' > a/b/c; printf 'k\n' > keep.txt
  git add -A; $G commit -qm one
  $G checkout -q -b other
  case "$kind" in
    del) git rm -q a/b/c ;;
    mod) printf 'v2\n' > a/b/c; git add -A ;;
    add) printf 'new\n' > a/new.txt; git add -A ;;
  esac
  $G commit -qm two
  $G checkout -q main
  git status --porcelain > /dev/null
}

# place the blocking ancestor: <dir> <ancestor> <outside>
block() {
  local d="$1" anc="$2" out="$3"
  case "$anc" in
    link_outside_empty)  mkdir -p "$out"; rm -rf "$d/a"; ln -s "$out" "$d/a" ;;
    link_outside_same)   mkdir -p "$out/b"; printf 'v1\n' > "$out/b/c"; rm -rf "$d/a"; ln -s "$out" "$d/a" ;;
    link_outside_target) mkdir -p "$out/b"; printf 'v2\n' > "$out/b/c"; rm -rf "$d/a"; ln -s "$out" "$d/a" ;;
    link_outside_dirty)  mkdir -p "$out/b"; printf 'zzz\n' > "$out/b/c"; rm -rf "$d/a"; ln -s "$out" "$d/a" ;;
    link_inside_same)    mkdir -p "$d/real/b"; printf 'v1\n' > "$d/real/b/c"; rm -rf "$d/a"; ln -s real "$d/a" ;;
    link_inside_empty)   mkdir -p "$d/real"; rm -rf "$d/a"; ln -s real "$d/a" ;;
    file)                rm -rf "$d/a"; printf 'blocker\n' > "$d/a" ;;
    missing)             rm -rf "$d/a" ;;
  esac
}

# run: <tag> <kind> <ancestor> <force: no|yes> <tool: git|mg>
run() {
  local tag="$1" kind="$2" anc="$3" force="$4" tool="$5"
  local base="$R/$tag-$tool" out="$R/$tag-$tool-outside"
  mkfix "$base" "$kind"
  block "$base/r" "$anc" "$out"
  local wt="$base/r"
  cd "$wt" || exit 1
  local cmd arg
  case "$kind" in
    del) cmd="switch";;
    mod) cmd="switch";;
    add) cmd="checkout";;
  esac
  local -a args
  if [ "$tool" = git ]; then
    args=($G "$cmd")
  else
    args=("$MG" "$cmd")
  fi
  if [ "$force" = yes ]; then args+=(-f); fi
  args+=(other)
  echo "===== $tag tool=$tool kind=$kind anc=$anc force=$force  [${args[*]}]"
  echo "before-status: $(git status --porcelain | tr '\n' '|')"
  "${args[@]}" > "$base/stdout" 2> "$base/stderr"
  echo "exit=$?"
  echo "stderr: $(tr '\n' '|' < "$base/stderr" | cut -c1-120)"
  echo "outside: $(cd "$out" 2>/dev/null && find . | sort | tr '\n' ' ')"
  echo "outside-b-c: $(cat "$out/b/c" 2>/dev/null || echo MISSING)"
  echo "a-type: $( [ -L "$wt/a" ] && echo symlink || ( [ -d "$wt/a" ] && echo dir || ([ -e "$wt/a" ] && echo file || echo missing) ) )"
  echo "a-target: $(readlink "$wt/a" 2>/dev/null || echo -)"
  echo "a-b-c: $(cat "$wt/a/b/c" 2>/dev/null || echo MISSING)"
  echo "newfile: $(cat "$wt/a/new.txt" 2>/dev/null || echo -)"
  echo "worktree: $(find . -path ./.git -prune -o -print | sed 's|^\./||' | sort | tr '\n' ' ')"
  echo "status: $(git status --porcelain | tr '\n' '|')"
  echo "lsfiles: $(git ls-files --stage | awk '{print $4}' | tr '\n' ' ')"
  echo "head: $(git rev-parse --abbrev-ref HEAD)"
}

for tool in git mg; do
  for anc in link_outside_empty link_outside_same link_outside_target link_inside_empty link_inside_same; do
    run A del "$anc" no  "$tool"
    run A del "$anc" yes "$tool"
    run A mod "$anc" no  "$tool"
    run A mod "$anc" yes "$tool"
    run A add "$anc" no  "$tool"
    run A add "$anc" yes "$tool"
  done
  for anc in file missing; do
    run A del "$anc" no  "$tool"
    run A del "$anc" yes "$tool"
    run A mod "$anc" no  "$tool"
    run A mod "$anc" yes "$tool"
    run A add "$anc" no  "$tool"
    run A add "$anc" yes "$tool"
  done
done
