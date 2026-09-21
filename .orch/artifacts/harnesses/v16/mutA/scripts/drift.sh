#!/usr/bin/env bash
# 对比当前工作区与某个快照，报告 CHANGED / ADDED / DELETED。
#
# 用法：scripts/drift.sh .orch/waves/W2/baseline.txt
# 退出码：0 = 无变化；1 = 有变化；2 = 快照不存在
#
# 这是「越界率」的工具：把结果与任务包里的 write scope 白名单对照，
# 落在白名单外的条目就是越界。

set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root" || exit 2

baseline="${1:-}"
if [ -z "$baseline" ] || [ ! -f "$baseline" ]; then
  echo "baseline snapshot not found: ${baseline:-<none>}" >&2
  exit 2
fi

tmp_before="$(mktemp)"
tmp_after="$(mktemp)"
trap 'rm -f "$tmp_before" "$tmp_after"' EXIT

sort "$baseline" > "$tmp_before"
"$root/scripts/snapshot.sh" | sort -k2 > "$tmp_after"

drift=0

while read -r hash path; do
  [ -z "${path:-}" ] && continue
  if ! grep -q "  $path\$" "$tmp_after"; then
    printf 'DELETED  %s\n' "$path"
    drift=$((drift + 1))
    continue
  fi
  now="$(grep "  $path\$" "$tmp_after" | head -1 | cut -d' ' -f1)"
  if [ "$now" != "$hash" ]; then
    printf 'CHANGED  %s\n' "$path"
    drift=$((drift + 1))
  fi
done < "$tmp_before"

while read -r hash path; do
  [ -z "${path:-}" ] && continue
  grep -q "  $path\$" "$tmp_before" || printf 'ADDED    %s\n' "$path"
done < "$tmp_after"

echo "---"
if [ "$drift" -gt 0 ]; then
  echo "changed/deleted pre-existing files: $drift"
  exit 1
fi
echo "no changes to pre-existing files (additions listed above, if any)"
exit 0
