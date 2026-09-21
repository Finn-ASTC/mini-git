#!/usr/bin/env bash
# V10 变异测试 v3 —— 修正 v2 的**陈旧编译产物**污染：
# v2 用 `cp -a`（保留 mtime）把干净源码盖回同一个 CARGO_TARGET_DIR，而该目录里
# 还留着上一轮「变异体编译」的产物；cargo 按 mtime 判定为 fresh，于是 baseline
# 实际跑的是**变异后的库**（baseline 19 failed），整批结果作废。
# 对策：全新 CARGO_TARGET_DIR + 每次运行前 `touch` 所有 .rs，强制重编被测 crate。
set -uo pipefail

SRC=/home/user/Projects/mini-git
WORK=/tmp/v10-mutant-v3
TARGET=/tmp/v10-target-mutant-v3
LOGS=/tmp/v10-mutant-logs4
TARGET_FILE=src/merge/three_way.rs

rm -rf "$LOGS" "$WORK"
mkdir -p "$LOGS" "$WORK"
for item in src tests scripts Cargo.toml Cargo.lock; do cp -a "$SRC/$item" "$WORK/"; done
cd "$WORK" || exit 2

cp -a "$TARGET_FILE" "$LOGS/three_way.rs.orig"
ORIG_SHA=$(sha256sum "$TARGET_FILE" | cut -d' ' -f1)
echo "test file sha256: $(sha256sum tests/verify_threeway.rs | cut -d' ' -f1)"
echo "src sha256      : $ORIG_SHA"
export CARGO_TARGET_DIR="$TARGET"

refresh_mtimes () { find src tests -name '*.rs' -exec touch {} + ; }

run_suite () {
  local name="$1"
  refresh_mtimes
  cargo test --offline --test verify_threeway > "$LOGS/$name.log" 2>&1
  local status=$?
  local result; result=$(grep -E '^test result:' "$LOGS/$name.log" | tail -1)
  echo "--- $name: exit=$status | $result"
  grep -E '^test .*FAILED' "$LOGS/$name.log" | sed -E 's/^test (.*) \.\.\. FAILED$/      \1/' | head -8
  return $status
}
restore () { cp -a "$LOGS/three_way.rs.orig" "$TARGET_FILE"; refresh_mtimes; }

echo "=== baseline（未变异，必须全绿）"
run_suite baseline
baseline_status=$?
if [ "$baseline_status" -ne 0 ]; then
  echo "!! baseline 不是全绿：后面的变异结果不可信，先修环境"
fi

mutate () { python3 - "$TARGET_FILE" "$@" <<'PY'
import sys
p, old, new, tag = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
s = open(p, encoding='utf-8').read()
assert s.count(old) == 1, f"{tag}: anchor count != 1 ({s.count(old)})"
open(p, 'w', encoding='utf-8').write(s.replace(old, new))
print(f"{tag} applied")
PY
}

echo
echo "=== M1 去掉二进制检测里的 theirs 分支"
mutate "if base.is_some_and(is_binary) || is_binary(ours) || is_binary(theirs) {" \
       "if base.is_some_and(is_binary) || is_binary(ours) {" "M1"
run_suite M1_drop_theirs_binary_check; restore

echo
echo "=== M2 让 merge_with_level 恒返回平凡值 Clean(ours)"
mutate ") -> Result<Merged> {
    // 1) 同一内容 / 只有一侧改动：直接取另一边（git 的 \`!xscr\` 短路）。" \
       ") -> Result<Merged> {
    let _ = (base, theirs, labels, level);
    return Ok(Merged::Clean(ours.to_vec()));
    // 1) 同一内容 / 只有一侧改动：直接取另一边（git 的 \`!xscr\` 短路）。" "M2"
run_suite M2_always_clean_ours; restore

echo
echo "=== M3 冲突 marker 宽度 7 → 6"
mutate "const MARKER_SIZE: usize = 7;" "const MARKER_SIZE: usize = 6;" "M3"
run_suite M3_marker_size_6; restore

echo
echo "=== M4 冲突判定恒为「干净」"
mutate "    if conflicts == 0 {
        Ok(Merged::Clean(out))
    } else {
        Ok(Merged::Conflict(out))
    }" "    let _ = conflicts;
    Ok(Merged::Clean(out))" "M4"
run_suite M4_conflicts_never_reported; restore

echo
AFTER_SHA=$(sha256sum "$TARGET_FILE" | cut -d' ' -f1)
echo "restored sha256: $AFTER_SHA"
[ "$AFTER_SHA" = "$ORIG_SHA" ] && echo "RESTORE_OK" || { echo "RESTORE_FAILED"; exit 3; }
echo "logs: $LOGS"
exit $baseline_status
