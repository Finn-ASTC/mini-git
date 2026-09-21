#!/usr/bin/env bash
# V8 证据脚本 1/2：生成真实 git 的 pkt-line 帧表（独立实现切片）。
# 用法: bash gen_evidence.sh <workdir>
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
W="${1:?usage: gen_evidence.sh <workdir>}"
rm -rf "$W"; mkdir -p "$W"

git -c init.defaultBranch=main init -q "$W/src"
printf 'hello v8\n' > "$W/src/a.txt"
git -C "$W/src" add a.txt
git -C "$W/src" -c user.name=v8-evidence -c user.email=v8@example.com -c commit.gpgsign=false commit -qm c1

python3 "$HERE/frame_table.py" "$W/src" > "$HERE/real-git-frames.txt"
echo "wrote $HERE/real-git-frames.txt"
