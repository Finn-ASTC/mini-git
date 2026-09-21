#!/usr/bin/env bash
# T13b 夹具矩阵：mg clone vs 真实 git clone
set -uo pipefail
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
MG=/home/user/Projects/mini-git/target/debug/mg
ROOT=/tmp/t13b/matrix
rm -rf "$ROOT"; mkdir -p "$ROOT"
ok=0; bad=0

mk_origin() { # $1=name $2=tags(none|light|annot|both) $3=detach(yes|no) $4=file_content
  local name=$1 tags=$2 detach=$3 content=$4
  local d="$ROOT/$name-origin"
  mkdir -p "$d"; (cd "$d" && git init -q -b main .)
  echo "$content" > "$d/a.txt"; mkdir -p "$d/sub"; echo nested > "$d/sub/n.txt"
  (cd "$d" && git add -A >/dev/null && git -c user.name=t -c user.email=t@e commit -qm one)
  (cd "$d" && echo "$content more" >> a.txt && git add -A >/dev/null && git -c user.name=t -c user.email=t@e commit -qm two)
  (cd "$d" && git branch feature >/dev/null)
  case "$tags" in
    light) (cd "$d" && git tag light >/dev/null) ;;
    annot) (cd "$d" && git -c user.name=t -c user.email=t@e tag -a v1 -m msg >/dev/null) ;;
    both)  (cd "$d" && git tag light >/dev/null && git -c user.name=t -c user.email=t@e tag -a v1 -m msg >/dev/null) ;;
  esac
  [ "$detach" = yes ] && (cd "$d" && git checkout -q --detach)
  echo "$d"
}

check() { # $1=label $2=origin $3=git_clone_extra_env(optional)
  local label=$1 origin=$2
  local mgd="$ROOT/$label-mg" gd="$ROOT/$label-git"
  echo "================ $label"
  if ! $MG clone "file://$origin" "$mgd" >"$ROOT/$label-mg.out" 2>&1; then
    echo "MG CLONE FAILED: $(cat "$ROOT/$label-mg.out")"; bad=$((bad+1)); return
  fi
  git clone -q "file://$origin" "$gd" || { echo "GIT CLONE FAILED"; bad=$((bad+1)); return; }
  local fail=0
  for cmd in "show-ref" "branch -a" "log --oneline" "status --porcelain"; do
    if ! diff <(git -C "$mgd" $cmd 2>&1 | sort) <(git -C "$gd" $cmd 2>&1 | sort) > "$ROOT/$label.diff" 2>&1; then
      echo "DIFF in [$cmd]:"; sed -n '1,12p' "$ROOT/$label.diff"; fail=1
    fi
  done
  for f in HEAD refs/remotes/origin/HEAD; do
    if ! diff <(cat "$mgd/.git/$f" 2>&1) <(cat "$gd/.git/$f" 2>&1) >/dev/null; then
      echo "DIFF in .git/$f: mg=$(cat "$mgd/.git/$f" 2>/dev/null) git=$(cat "$gd/.git/$f" 2>/dev/null)"; fail=1
    fi
  done
  for key in "remote.origin.url" "remote.origin.fetch" "branch.main.remote" "branch.main.merge" "core.bare"; do
    m=$(git -C "$mgd" config --get "$key"); g=$(git -C "$gd" config --get "$key")
    [ "$m" != "$g" ] && { echo "CONFIG DIFF $key: mg='$m' git='$g'"; [ "$key" = "remote.origin.url" ] && fail=1; }
  done
  if ! git -C "$mgd" fsck --no-progress >"$ROOT/$label.fsck" 2>&1; then
    echo "FSCK FAILED:"; sed -n '1,6p' "$ROOT/$label.fsck"; fail=1
  fi
  if [ "$fail" = 0 ]; then echo "OK (show-ref / branch -a / log / status / HEAD / origin-HEAD / fsck 全部与真实 git clone 一致)"; ok=$((ok+1)); else bad=$((bad+1)); fi
}

check no-tags     "$(mk_origin notags none  no  alpha)"
check light-only  "$(mk_origin light  light no  beta)"
check annot-only  "$(mk_origin annot  annot no  gamma)"
check both-tags   "$(mk_origin both   both  no  delta)"
check detached    "$(mk_origin detach both  yes epsilon)"
check detached-notags "$(mk_origin detach2 none yes zeta)"
echo "================ 小结: ok=$ok bad=$bad"
