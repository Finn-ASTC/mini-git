#!/usr/bin/env bash
# 接口漂移检测：把当前文件哈希与 .orch/FREEZE-v0.md 中的基线逐项对比。
#
# 用法：scripts/check-freeze.sh
# 退出码：0 = 无漂移；1 = 有改动/缺失；2 = 基线缺失（沙箱/克隆不完整）
#
# 它是 ORCHESTRATION.md §6「接口漂移次数」指标的执行工具：
# 每个 wave 结束时跑一次，记录 changed/missing 的条目。

set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$root/.orch/FREEZE-v0.md"

if [ ! -f "$manifest" ]; then
  echo "freeze manifest not found: $manifest" >&2
  exit 2
fi

cd "$root" || exit 2

drift=0
checked=0

while read -r file want; do
  [ -z "${file:-}" ] && continue
  checked=$((checked + 1))
  if [ ! -f "$file" ]; then
    printf 'MISSING  %s\n' "$file"
    drift=$((drift + 1))
    continue
  fi
  got="$(sha256sum "$file" | cut -c1-16)"
  if [ "$got" != "$want" ]; then
    printf 'CHANGED  %s\n         baseline=%s now=%s\n' "$file" "$want" "$got"
    drift=$((drift + 1))
  fi
done < <(awk -F'|' '
  /^\| `/ {
    f = $2; h = $3;
    gsub(/[` \t]/, "", f);
    gsub(/[` \t]/, "", h);
    if (f != "" && h != "") print f, h;
  }' "$manifest")

echo "---"
echo "checked $checked file(s), drift $drift"

if [ "$drift" -eq 0 ]; then
  echo "freeze-v0 intact"
  exit 0
fi

echo "interface drift detected: 若为有意改动，请由 controller 更新 FREEZE-v0.md 并记录原因" >&2
exit 1
