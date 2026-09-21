#!/usr/bin/env bash
# 为一个 wave 拍开工前快照（默认输出到 stdout 的 `sha256  path` 文本）。
#
# 用法：scripts/snapshot.sh > .orch/waves/W2/baseline.txt
#      scripts/snapshot.sh /path/to/baseline.txt
#
# 与 scripts/check-freeze.sh 的区别：check-freeze 检查的是「W0 冻结的公共接口」，
# 而本脚本拍的是「这一轮开工时的工作区状态」，用来回答
# 「这一轮到底改动了哪些文件」——也就是越界率（ORCHESTRATION.md §6）。

set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root" || exit 2

out="${1:-}"

emit() {
  find src tests scripts .orch -type f \
    ! -path '.orch/rounds/*' \
    ! -path '*/verify-scratch/*' \
    ! -name '*.tmp' \
    | sort | xargs sha256sum
  printf '%s\n' "Cargo.toml Cargo.lock" | tr ' ' '\n' | while read -r f; do
    [ -f "$f" ] && sha256sum "$f"
  done
}

if [ -n "$out" ]; then
  emit > "$out"
  echo "snapshot written: $out ($(wc -l < "$out") files)"
else
  emit
fi
