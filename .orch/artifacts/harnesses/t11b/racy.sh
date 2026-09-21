#!/bin/bash
# Does git's refusal depend on racy stat or on the mere existence of a file behind the symlink?
set -u
R=/tmp/t11b/racy
G="git -c user.name=t -c user.email=t@e"
rm -rf "$R"; mkdir -p "$R"

# mkfix <dir> <kind>
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
  esac
  $G commit -qm two
  $G checkout -q main
  git status --porcelain > /dev/null
}

# run <tag> <kind> <leaf: same|same-old|dirty|absent> <anc: outside|inside> [extra-touch]
run() {
  local tag="$1" kind="$2" leaf="$3" anc="$4"
  local base="$R/$tag" out="$R/$tag-outside"
  mkfix "$base" "$kind"
  local wt="$base/r"
  mkdir -p "$out/b"
  if [ "$leaf" = same ] || [ "$leaf" = same-old ]; then printf 'v1\n' > "$out/b/c"; fi
  if [ "$leaf" = dirty ]; then printf 'v2\n' > "$out/b/c"; fi
  if [ "$leaf" = absent ]; then rmdir "$out/b"; fi
  if [ "$leaf" = same-old ]; then touch -d '2000-01-01 00:00:00' "$out/b/c"; fi
  if [ "$anc" = outside ]; then rm -rf "$wt/a"; ln -s "$out" "$wt/a"; else
    mkdir -p "$wt/real/b"; if [ "$leaf" != absent ]; then printf 'v1\n' > "$wt/real/b/c"; fi
    if [ "$leaf" = dirty ]; then printf 'v2\n' > "$wt/real/b/c"; fi
    if [ "$leaf" = same-old ]; then touch -d '2000-01-01 00:00:00' "$wt/real/b/c"; fi
    rm -rf "$wt/a"; ln -s real "$wt/a"
  fi
  cd "$wt" || exit 1
  echo "===== $tag kind=$kind leaf=$leaf anc=$anc"
  echo "  leaf-lstat: $(stat -c '%s %Y' "$wt/a/b/c" 2>/dev/null || echo UNREACHABLE)"
  $G switch other > "$base/o" 2> "$base/e"
  echo "  exit=$? stderr=$(tr '\n' '|' < "$base/e" | cut -c1-90)"
  echo "  a-type=$( [ -L "$wt/a" ] && echo symlink || ([ -d "$wt/a" ] && echo dir || echo other) ) a-b-c=$(cat "$wt/a/b/c" 2>/dev/null || echo MISSING)"
  echo "  outside=$(cd "$out" && find . | sort | tr '\n' ' ')"
  echo "  status=$(git status --porcelain | tr '\n' '|') lsfiles=$(git ls-files --stage | awk '{print $4}' | tr '\n' ' ')"
}

for kind in del mod; do
  for leaf in absent same same-old dirty; do
    for anc in outside inside; do
      run "$kind-$leaf-$anc" "$kind" "$leaf" "$anc"
    done
  done
done

# Also: same content + old mtime, but the leaf reachable via a *relative* symlink outside
run rel-same-old del same-old outside
