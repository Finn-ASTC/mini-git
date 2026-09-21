#!/usr/bin/env bash
# V10 变异测试：在**独立副本 + 独立 CARGO_TARGET_DIR** 上跑，绝不修改共享仓库
# /home/user/Projects/mini-git 里的 src/** 与共享 target/。
#
# 用法： bash /home/user/Projects/mini-git/.orch/waves/W3/T10-threeway/verify-scratch/mutants.sh
# 产物： /tmp/v10-mutant-logs/*.log  +  stdout 上的「变异体 → 是否被检出」摘要

set -uo pipefail

SRC=/home/user/Projects/mini-git
WORK=/tmp/v10-mutant
TARGET=/tmp/v10-target-mutant
LOGS=/tmp/v10-mutant-logs
TARGET_FILE=src/merge/three_way.rs

rm -rf "$WORK" "$TARGET" "$LOGS"
mkdir -p "$WORK" "$LOGS"

for item in src tests scripts Cargo.toml Cargo.lock; do
  cp -a "$SRC/$item" "$WORK/"
done
cd "$WORK" || exit 2

# 备份原始文件，结束后必须还原。
cp -a "$TARGET_FILE" "$LOGS/three_way.rs.orig"
ORIG_SHA=$(sha256sum "$TARGET_FILE" | cut -d' ' -f1)
echo "original sha256: $ORIG_SHA"

export CARGO_TARGET_DIR="$TARGET"

run_suite () {
  local name="$1"
  cargo test --offline --test verify_threeway > "$LOGS/$name.log" 2>&1
  local status=$?
  echo "--- $name: exit=$status"
  grep -E "^test .*(FAILED|ok)$" "$LOGS/$name.log" | grep -c "FAILED" | sed 's/^/    failing tests: /'
  grep -E "^test .*FAILED" "$LOGS/$name.log" | sed 's/^/    /' | head -12
  grep -E "^test result:" "$LOGS/$name.log" | sed 's/^/    /'
  return $status
}

restore () {
  cp -a "$LOGS/three_way.rs.orig" "$TARGET_FILE"
}

echo "=========== baseline (未变异，必须全绿) ==========="
run_suite baseline
baseline_status=$?

echo
echo "=========== 变异体 M1：去掉二进制检测里的 theirs 检查 ==========="
python3 - "$TARGET_FILE" <<'PY'
import sys
p = sys.argv[1]
s = open(p, encoding='utf-8').read()
old = "if base.is_some_and(is_binary) || is_binary(ours) || is_binary(theirs) {"
new = "if base.is_some_and(is_binary) || is_binary(ours) {"
assert s.count(old) == 1, "M1 anchor not found exactly once"
open(p, 'w', encoding='utf-8').write(s.replace(old, new))
print("M1 applied")
PY
run_suite mutant1_binary_check_removed
restore

echo
echo "=========== 变异体 M2：让实现恒返回平凡值 Clean(ours) ==========="
python3 - "$TARGET_FILE" <<'PY'
import sys
p = sys.argv[1]
s = open(p, encoding='utf-8').read()
anchor = """    labels: &MergeLabels,
    level: i32,
) -> Result<Merged> {
"""
new = anchor + "    let _ = (base, theirs, labels, level);\n    return Ok(Merged::Clean(ours.to_vec()));\n"
assert s.count(anchor) == 1, "M2 anchor not found exactly once"
open(p, 'w', encoding='utf-8').write(s.replace(anchor, new))
print("M2 applied")
PY
run_suite mutant2_always_clean_ours
restore

echo
echo "=========== 变异体 M3：冲突 marker 宽度 7 → 6 ==========="
python3 - "$TARGET_FILE" <<'PY'
import sys
p = sys.argv[1]
s = open(p, encoding='utf-8').read()
old = "const MARKER_SIZE: usize = 7;"
new = "const MARKER_SIZE: usize = 6;"
assert s.count(old) == 1, "M3 anchor not found exactly once"
open(p, 'w', encoding='utf-8').write(s.replace(old, new))
print("M3 applied")
PY
run_suite mutant3_marker_size_6
restore

echo
AFTER_SHA=$(sha256sum "$TARGET_FILE" | cut -d' ' -f1)
echo "restored sha256: $AFTER_SHA"
if [ "$AFTER_SHA" = "$ORIG_SHA" ]; then
  echo "RESTORE_OK"
else
  echo "RESTORE_FAILED"
  exit 3
fi

echo
echo "=========== 摘要（logs in $LOGS） ==========="
for f in baseline mutant1_binary_check_removed mutant2_always_clean_ours mutant3_marker_size_6; do
  printf '%-36s %s\n' "$f" "$(grep -E '^test result:' "$LOGS/$f.log" | tail -1)"
done
exit $baseline_status
