#!/usr/bin/env bash
# V10 变异测试 v2（验证者自跑）：修正 M2 锚点（v1 的锚点在源文件里出现两次，
# 导致 M2 根本没被应用）。用**独立副本 + 独立 CARGO_TARGET_DIR**，绝不碰共享仓库。
set -uo pipefail

SRC=/home/user/Projects/mini-git
WORK=/tmp/v10-mutant
TARGET=/tmp/v10-target-mutant
LOGS=/tmp/v10-mutant-logs2
TARGET_FILE=src/merge/three_way.rs

rm -rf "$LOGS"; mkdir -p "$LOGS" "$WORK"
# 重置副本：源码 + 最终版测试（每次都以仓库当前状态为准）
rm -rf "$WORK/src" "$WORK/tests" "$WORK/scripts"
for item in src tests scripts Cargo.toml Cargo.lock; do cp -a "$SRC/$item" "$WORK/"; done
cd "$WORK" || exit 2

cp -a "$TARGET_FILE" "$LOGS/three_way.rs.orig"
ORIG_SHA=$(sha256sum "$TARGET_FILE" | cut -d' ' -f1)
echo "copied test file sha256: $(sha256sum tests/verify_threeway.rs | cut -d' ' -f1)"
echo "original src sha256    : $ORIG_SHA"
export CARGO_TARGET_DIR="$TARGET"

run_suite () {
  local name="$1"
  cargo test --offline --test verify_threeway > "$LOGS/$name.log" 2>&1
  local status=$?
  local result; result=$(grep -E '^test result:' "$LOGS/$name.log" | tail -1)
  echo "--- $name: exit=$status | $result"
  echo "    FAILED tests:"
  grep -E '^test .*FAILED' "$LOGS/$name.log" | sed -E 's/^test (.*) \.\.\. FAILED$/      \1/' | head -8
  return $status
}
restore () { cp -a "$LOGS/three_way.rs.orig" "$TARGET_FILE"; }

echo "=== baseline（未变异，必须全绿）"
run_suite baseline

echo
echo "=== M1 去掉二进制检测中的 theirs 分支（删掉一条检查）"
python3 - "$TARGET_FILE" <<'PY'
import sys
p=sys.argv[1]; s=open(p,encoding='utf-8').read()
old="if base.is_some_and(is_binary) || is_binary(ours) || is_binary(theirs) {"
new="if base.is_some_and(is_binary) || is_binary(ours) {"
assert s.count(old)==1, "M1 anchor count != 1"
open(p,'w',encoding='utf-8').write(s.replace(old,new)); print("M1 applied")
PY
run_suite M1_drop_theirs_binary_check
restore

echo
echo "=== M2 让 merge_with_level 恒返回平凡值 Clean(ours)（首行短路）"
python3 - "$TARGET_FILE" <<'PY'
import sys
p=sys.argv[1]; s=open(p,encoding='utf-8').read()
anchor=") -> Result<Merged> {\n    // 1) 同一内容 / 只有一侧改动：直接取另一边（git 的 `!xscr` 短路）。\n"
assert s.count(anchor)==1, "M2 anchor count != 1"
new=") -> Result<Merged> {\n    return Ok(Merged::Clean(ours.to_vec()));\n    // 1) 同一内容 / 只有一侧改动：直接取另一边（git 的 `!xscr` 短路）。\n"
open(p,'w',encoding='utf-8').write(s.replace(anchor,new)); print("M2 applied")
PY
run_suite M2_always_clean_ours
restore

echo
echo "=== M3 冲突 marker 宽度 7 → 6"
python3 - "$TARGET_FILE" <<'PY'
import sys
p=sys.argv[1]; s=open(p,encoding='utf-8').read()
old="const MARKER_SIZE: usize = 7;"; new="const MARKER_SIZE: usize = 6;"
assert s.count(old)==1, "M3 anchor count != 1"
open(p,'w',encoding='utf-8').write(s.replace(old,new)); print("M3 applied")
PY
run_suite M3_marker_size_6
restore

echo
echo "=== M4 冲突判定恒为「干净」（conflicts>0 也返回 Clean）"
python3 - "$TARGET_FILE" <<'PY'
import sys
p=sys.argv[1]; s=open(p,encoding='utf-8').read()
old="""    if conflicts == 0 {
        Ok(Merged::Clean(out))
    } else {
        Ok(Merged::Conflict(out))
    }"""
new="""    let _ = conflicts;
    Ok(Merged::Clean(out))"""
assert s.count(old)==1, "M4 anchor count != 1"
open(p,'w',encoding='utf-8').write(s.replace(old,new)); print("M4 applied")
PY
run_suite M4_conflicts_never_reported
restore

AFTER_SHA=$(sha256sum "$TARGET_FILE" | cut -d' ' -f1)
echo
echo "restored sha256: $AFTER_SHA"
[ "$AFTER_SHA" = "$ORIG_SHA" ] && echo "RESTORE_OK" || { echo "RESTORE_FAILED"; exit 3; }
echo "logs: $LOGS"
