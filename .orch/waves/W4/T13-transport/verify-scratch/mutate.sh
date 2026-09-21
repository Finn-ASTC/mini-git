#!/usr/bin/env bash
# V13 变异测试夹具：在**独立副本 + 独立 CARGO_TARGET_DIR** 上注入一个变异体，再跑
# 本轮验证者写的 tests/verify_transport.rs。
#
# 用法：bash .orch/waves/W4/T13-transport/verify-scratch/mutate.sh <1|2|3>
#   1 = 语义变异：`expand_one` 的通配 prefix/suffix 匹配 → 等值匹配（refspec 的通配全失效）
#   2 = 平凡值变异：`build_pack` 恒返回空 Vec（fetch/push 永远不传对象）
#   3 = 语义变异：去掉 `push_with` 的客户端 non-fast-forward 拒绝
#
# 不改本仓库的 src/**，也不污染共享 target/。
set -uo pipefail

n="${1:?usage: mutate.sh <1|2|3>}"
root=/home/user/Projects/mini-git
work="/tmp/v13mut/m${n}"
target="/tmp/v13mut/target-m${n}"
log="/tmp/v13mut/log-m${n}.txt"
mkdir -p /tmp/v13mut
rm -rf "$work"
mkdir -p "$work"
cp -a "$root/src" "$root/tests" "$root/Cargo.toml" "$root/Cargo.lock" "$work/"
cd "$work" || exit 2

python3 - "$n" <<'PY'
import sys

n = sys.argv[1]
path = "src/transport/local.rs"
src = open(path).read()

if n == "1":
    old = """        return Ok(advertised
            .iter()
            .filter(|(name, _)| {
                name.len() >= prefix.len() + suffix.len()
                    && name.starts_with(prefix)
                    && name.ends_with(suffix)
            })
            .cloned()
            .collect());"""
    new = """        return Ok(advertised
            .iter()
            .filter(|(name, _)| name == src)
            .cloned()
            .collect());"""
elif n == "2":
    old = """fn build_pack(repo: &Repo, oids: &BTreeSet<Oid>) -> Result<Vec<u8>> {
    let odb = Odb::new(repo);"""
    new = """fn build_pack(repo: &Repo, oids: &BTreeSet<Oid>) -> Result<Vec<u8>> {
    let _ = (repo, oids);
    return Ok(Vec::new());
    #[allow(unreachable_code)]
    let odb = Odb::new(repo);"""
elif n == "3":
    old = "        if !force && !old.is_zero() {"
    new = "        if false && !force && !old.is_zero() {"
else:
    sys.exit(f"unknown mutant {n}")

count = src.count(old)
if count != 1:
    sys.exit(f"mutant {n}: pattern matched {count} times, refusing to patch")
open(path, "w").write(src.replace(old, new, 1))
print(f"mutant {n} applied to {path}")
PY
patch_status=$?
if [ "$patch_status" -ne 0 ]; then
  echo "PATCH-FAILED"
  exit 3
fi

{
  echo "### mutant $n : cargo test --offline --test verify_transport (CARGO_TARGET_DIR=$target)"
  CARGO_TARGET_DIR="$target" cargo test --offline --test verify_transport 2>&1
  echo "CARGO-EXIT=$?"
} >"$log" 2>&1
cat "$log"
