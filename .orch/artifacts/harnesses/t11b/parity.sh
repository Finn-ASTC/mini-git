#!/bin/bash
# §3.3 平行仓库逐字节对拍：mg（A） vs 真实 git（B）。
# 对拍项：工作区内容 + 可执行位 + symlink 目标、git status --porcelain、ls-files --stage、
#         HEAD / .git/HEAD、工作区外的 sha256 清单、退出码。
set -u
R=/tmp/t11b/parity
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c user.name=t -c user.email=t@e"
rm -rf "$R"; mkdir -p "$R"
FAILED=0
CHECKED=0

# 两侧的「工作区外目录」绝对路径不同，对拍时归一化成 OUTSIDE
dump_work() {
  cd "$1" || return
  find . -path ./.git -prune -o -print | LC_ALL=C sort | while IFS= read -r p; do
    [ "$p" = "." ] && continue
    if [ -L "$p" ]; then printf 'L %s -> %s\n' "$p" "$(readlink "$p" | sed -e "s|$2|OUTSIDE|g")"
    elif [ -d "$p" ]; then printf 'D %s\n' "$p"
    else printf 'F %s %s %s\n' "$p" "$(sha256sum < "$p" | cut -c1-16)" "$(stat -c %a "$p")"; fi
  done
}

dump_outside() {
  [ -d "$1" ] || return
  cd "$1" || return
  find . | LC_ALL=C sort | while IFS= read -r p; do
    [ "$p" = "." ] && continue
    if [ -L "$p" ]; then printf 'L %s -> %s\n' "$p" "$(readlink "$p")"
    elif [ -d "$p" ]; then printf 'D %s\n' "$p"
    else printf 'F %s %s\n' "$p" "$(sha256sum < "$p" | cut -c1-16)"; fi
  done
}

# fix <dir> <kind: prunes|modify>   生成夹具（用真实 git），并带上 other / prune 分支
fix() {
  local d="$1" kind="$2"
  rm -rf "$d"; mkdir -p "$d"; cd "$d" || exit 1
  $G init -q -b main r; cd r || exit 1
  mkdir -p a/b; printf 'X\n' > a/b/c; printf 'k\n' > keep.txt; printf 'n\n' > other.txt
  git add -A; $G commit -qm one
  printf 'g\n' > gone.txt
  git add -A; $G commit -qm extra
  $G branch prune
  $G switch -q prune
  if [ "$kind" = batch ]; then git rm -q a/b/c gone.txt; else git rm -q a/b/c; fi
  $G commit -qm prune
  $G switch -q main
  $G branch other
  $G switch -q other
  case "$kind" in
    modify) printf 'MODIFIED\n' > a/b/c; git add -A; $G commit -qm modify ;;
    add)    printf 'new\n' > a/new.txt; git add -A; $G commit -qm add ;;
  esac
  $G switch -q main
  git status --porcelain > /dev/null
}

# run <tag> <kind> <anc: outside|inside> <cmd...>
run() {
  local tag="$1" kind="$2" anc="$3"; shift 3
  local base="$R/$tag"
  fix "$base" "$kind"
  for side in a b; do
    cp -a "$base/r" "$base/$side"
    local out="$base/$side-outside"
    case "$anc" in
      outside)
        mkdir -p "$out/b"; printf 'X\n' > "$out/b/c"
        rm -rf "$base/$side/a"; ln -s "$out" "$base/$side/a" ;;
      outside-empty)
        mkdir -p "$out"
        rm -rf "$base/$side/a"; ln -s "$out" "$base/$side/a" ;;
      inside)
        mkdir -p "$base/$side/real/b"; printf 'X\n' > "$base/$side/real/b/c"
        rm -rf "$base/$side/a"; ln -s real "$base/$side/a" ;;
    esac
  done
  local before_outside_a before_outside_b
  before_outside_a="$(dump_outside "$base/a-outside")"
  before_outside_b="$(dump_outside "$base/b-outside")"

  ( cd "$base/a" && "$MG" "$@" > "$base/a.stdout" 2> "$base/a.stderr" ); local rc_a=$?
  ( cd "$base/b" && $G "$@" > "$base/b.stdout" 2> "$base/b.stderr" ); local rc_b=$?
  CHECKED=$((CHECKED+1))

  local wa wb oa ob sa sb la lb ha hb
  wa="$(dump_work "$base/a" "$base/a-outside")"; wb="$(dump_work "$base/b" "$base/b-outside")"
  oa="$(dump_outside "$base/a-outside")"; ob="$(dump_outside "$base/b-outside")"
  sa="$($G -C "$base/a" status --porcelain)"; sb="$($G -C "$base/b" status --porcelain)"
  la="$($G -C "$base/a" ls-files --stage)"; lb="$($G -C "$base/b" ls-files --stage)"
  ha="$(cat "$base/a/.git/HEAD"; $G -C "$base/a" rev-parse HEAD 2>/dev/null)"
  hb="$(cat "$base/b/.git/HEAD"; $G -C "$base/b" rev-parse HEAD 2>/dev/null)"

  local ok=1
  [ "$rc_a" != "$rc_b" ] && ok=0
  [ "$wa" != "$wb" ] && ok=0
  [ "$oa" != "$ob" ] && ok=0
  [ "$sa" != "$sb" ] && ok=0
  [ "$la" != "$lb" ] && ok=0
  [ "$ha" != "$hb" ] && ok=0
  [ "$oa" != "$before_outside_a" ] && ok=0

  if [ "$ok" = 1 ]; then
    printf 'T11b-PARITY OK   %-28s exit(mg=%s git=%s) outside-unchanged=%s\n' \
      "$tag [$*]" "$rc_a" "$rc_b" "$([ "$oa" = "$before_outside_a" ] && echo yes || echo NO)"
  else
    FAILED=$((FAILED+1))
    printf 'T11b-PARITY FAIL %s [%s]\n' "$tag" "$*"
    for f in rc:rc_a:rc_b work:wa:wb outside:oa:ob status:sa:sb lsfiles:la:lb head:ha:hb; do
      IFS=: read -r label x y <<<"$f"
      local va vb
      eval "va=\$$x"; eval "vb=\$$y"
      [ "$va" != "$vb" ] && { printf '  --- %s\n     mg : %s\n     git: %s\n' "$label" "$va" "$vb"; }
    done
    [ "$oa" != "$before_outside_a" ] && printf '  --- mg 破坏了工作区外: 之前=%s 之后=%s\n' "$before_outside_a" "$oa"
    printf '     mg stderr: %s\n' "$(tr '\n' '|' < "$base/a.stderr")"
  fi
}

echo "=== 组① FAIL 1：删除路径 + 指向工作区外的 symlink 祖先（三条命令）"
for c in "switch prune" "switch -f prune" "checkout -f prune" "reset --hard prune" "checkout prune"; do
  run "g1-$(echo "$c" | tr ' -' '__')" prunes outside $c
done
echo "=== 组② FAIL 2：写出路径 + 指向工作区外的 symlink 祖先（目标不存在）"
for c in "switch other" "checkout other" "checkout -f other" "reset --hard other" "switch -f other"; do
  run "g2-$(echo "$c" | tr ' -' '__')" modify outside $c
done
echo "=== 组②b FAIL 2：目标新增文件（未跟踪路径不得被 symlink 祖先放行）"
for c in "switch other" "switch -f other"; do
  run "g2b-$(echo "$c" | tr ' -' '__')" add outside $c
done
echo "=== 组③ symlink 祖先指向工作区内目录：删除与写入"
echo "=== 组②c FAIL 2 原场景：symlink 指向空的（顺它 stat 不到目标的）工作区外目录"
for c in "switch other" "checkout other" "switch -f other" "reset --hard other"; do
  run "g2c-$(echo "$c" | tr ' -' '__')" modify outside-empty $c
done
echo "=== 组③ symlink 祖先指向工作区内目录：删除与写入"
echo "=== 组④ 批量删除：被 symlink 挡住的路径跳过，其它路径照删（force=硬重置）"
for c in "reset --hard prune" "switch -f prune"; do
  run "g4-$(echo "$c" | tr ' -' '__')" batch outside $c
done
echo "=== 组③ symlink 祖先指向工作区内目录：删除与写入"
run g3-del-no  prunes inside switch prune
run g3-del-f   prunes inside switch -f prune
run g3-write-no modify inside switch other
run g3-write-f  modify inside switch -f other

echo "----"
echo "checked=$CHECKED failed=$FAILED"
exit $FAILED
