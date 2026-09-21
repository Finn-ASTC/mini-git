#!/usr/bin/env bash
# V8 证据脚本 2/2：变异测试（证明 V8 的独立测试不是「假绿」）。
#
# 纪律：绝不修改共享 checkout（src/** 只读）。做法是把工作树复制到 /tmp，
# 并用独立的 CARGO_TARGET_DIR，避免污染共享 target/（W1 的 V2 踩过）。
#
# 用法: bash mutate.sh /home/user/Projects/mini-git
set -uo pipefail
SRC_REPO="${1:?usage: mutate.sh <shared-checkout>}"
WORK=/tmp/v8-mutant
TARGET=/tmp/v8-mutant-target
OUT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/mutation.txt"
PKT=src/transport/pktline.rs

run_tests() { # $1 = 标签
  local label="$1" mine theirs
  if (cd "$WORK" && CARGO_TARGET_DIR="$TARGET" cargo test --offline --test verify_pktline >/tmp/v8-mut-mine.log 2>&1); then
    mine="PASS(未检出)"
  else
    if grep -qE "^error(\[|:)" /tmp/v8-mut-mine.log && ! grep -q "test result" /tmp/v8-mut-mine.log; then
      mine="COMPILE_ERROR"
    else
      mine="FAIL(检出: $(grep -cE '^test .* FAILED' /tmp/v8-mut-mine.log) 个用例失败)"
    fi
  fi
  if (cd "$WORK" && CARGO_TARGET_DIR="$TARGET" cargo test --offline --lib transport::pktline >/tmp/v8-mut-theirs.log 2>&1); then
    theirs="PASS(未检出)"
  else
    if grep -q "test result" /tmp/v8-mut-theirs.log; then
      theirs="FAIL(检出: $(grep -cE '^test .* FAILED' /tmp/v8-mut-theirs.log) 个用例失败)"
    else
      theirs="COMPILE_ERROR"
    fi
  fi
  printf '%-46s | V8 verify_pktline: %-22s | 作者自带 lib 测试: %s\n' "$label" "$mine" "$theirs" | tee -a "$OUT"
}

rm -rf "$WORK"; mkdir -p "$WORK"
cp -a "$SRC_REPO/src" "$SRC_REPO/tests" "$SRC_REPO/Cargo.toml" "$SRC_REPO/Cargo.lock" "$WORK/"
cp -a "$SRC_REPO/$PKT" /tmp/v8-pktline-pristine.rs

{
  echo "V8 变异测试（独立副本 $WORK，独立 CARGO_TARGET_DIR=$TARGET）"
  echo "基线 pktline.rs sha256: $(sha256sum /tmp/v8-pktline-pristine.rs | cut -d' ' -f1)"
  echo "共享 checkout pktline.rs sha256: $(sha256sum "$SRC_REPO/$PKT" | cut -d' ' -f1)"
  echo "git: $(git --version)"
  echo
} > "$OUT"

echo "== 基线（未变异）==" | tee -a "$OUT"
run_tests "baseline（未变异）"

mutate() { # $1 = 标签, $2 = python 替换代码
  cp -a /tmp/v8-pktline-pristine.rs "$WORK/$PKT"
  python3 - "$WORK/$PKT" <<PY
import sys
p = sys.argv[1]
s = open(p).read()
$2
open(p, "w").write(s)
PY
  run_tests "$1"
}

mutate "M1 去掉帧长上限（MAX_FRAME_LEN=usize::MAX）" \
  's = s.replace("const MAX_FRAME_LEN: usize = MAX_PAYLOAD_LEN + 4;",
                "const MAX_FRAME_LEN: usize = usize::MAX; // MUTANT")
assert "MUTANT" in s'

mutate "M2 截断时返回 Error::Io 而非 Protocol" \
  's = s.replace("""        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Err(Error::Protocol(
            format!(\"truncated stream while reading {what}\"),
        )),
        Err(err) => Err(Error::Io(err)),""",
                "        Err(err) => Err(Error::Io(err)), // MUTANT")
assert "MUTANT" in s'

mutate "M3 write_pkt / write_flush 不调用 flush()" \
  's = s.replace("    writer.write_all(&encode_pkt(data))?;\n    writer.flush()?;",
                "    writer.write_all(&encode_pkt(data))?; // MUTANT: no flush")
s = s.replace("    writer.write_all(b\"0000\")?;\n    writer.flush()?;",
              "    writer.write_all(b\"0000\")?; // MUTANT: no flush")
assert s.count("MUTANT") == 2'

mutate "M4 write_pkt 写错长度（len 而不是 len+4）" \
  's = s.replace("    writer.write_all(&encode_pkt(data))?;",
                "    let mut raw = format!(\"{:04x}\", data.len()).into_bytes();\n    raw.extend_from_slice(data);\n    writer.write_all(&raw)?; // MUTANT")
assert "MUTANT" in s'

mutate "M5 read_pkt 读 payload 时吃掉整个流（read_to_end）" \
  's = s.replace("use std::io::{BufRead, Write};", "use std::io::{BufRead, Read, Write};")
s = s.replace("    read_exact_protocol(reader, &mut payload, \"pkt-line payload\")?;",
              "    reader.read_to_end(&mut payload)?; // MUTANT")
assert "MUTANT" in s'

mutate "M6 0000 不再立即返回 Flush（当作 Data 处理）" \
  's = s.replace("        0 => return Ok(Pkt::Flush),", "        0 => {} // MUTANT")
assert "MUTANT" in s'

cp -a /tmp/v8-pktline-pristine.rs "$WORK/$PKT"
echo
echo "（副本已恢复原始内容；结果写入 $OUT）"
