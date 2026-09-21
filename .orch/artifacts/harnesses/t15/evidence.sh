#!/usr/bin/env bash
# T15 final evidence: the three mandated manual scenarios + multi-pack + bare repo.
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c gc.auto=0 -c init.defaultBranch=main -c user.name=A -c user.email=a@e.com -c protocol.file.allow=always -c commit.gpgsign=false"
W=/tmp/t15/evidence; rm -rf "$W"; mkdir -p "$W"
counts() { $G count-objects -v | awk 'NR<=5{printf "%s=%s ", $1, $2} END{print ""}' | tr -d ':'; }

echo "########## SCENARIO 1: 真实 git 仓库（含 packfile）→ mg fsck 全绿"
R=$W/s1; mkdir -p "$R"; cd "$R"
$G init -q .
mkdir -p sub/deep
printf 'one\n' > a.txt; printf 'two\n' > sub/b.txt; printf 'three\n' > sub/deep/c.txt; printf 'space\n' > 'with space.txt'
$G add -A; $G commit -qm c1
printf 'one v2\n' > a.txt; $G add -A; $G commit -qm c2
$G tag -a v1 -m annotated; $G tag light; $G branch feature
$G gc -q
echo "packs:        $(ls .git/objects/pack/ | tr '\n' ' ')"
$MG fsck;           echo "mg fsck        exit=$?"
$MG fsck --full;    echo "mg fsck --full exit=$?"
$G fsck --no-progress; echo "git fsck       exit=$?"
printf 'dangling\n' | $G hash-object -w --stdin >/dev/null
$MG fsck;           echo "mg fsck (with a dangling object) exit=$?"

echo
echo "########## SCENARIO 2: mg gc 一个只有 loose 对象的仓库"
R=$W/s2; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null
mkdir dir; printf 'alpha\n' > a.txt; printf 'beta\n' > dir/b.txt
$MG add . >/dev/null; $MG commit -m first >/dev/null
printf 'alpha v2\n' > a.txt; $MG add a.txt >/dev/null; $MG commit -m second >/dev/null
$MG tag v1 >/dev/null; $MG tag -a va -m 'annotated tag' >/dev/null; $MG branch side >/dev/null
echo "count-objects BEFORE:  $(counts)"
$G cat-file --batch-all-objects --batch-check | sort > $W/objs.before
$G log --all --oneline > $W/log.before
echo "mg fsck before gc:     exit=$($MG fsck; echo $?)"
$MG gc; echo "mg gc                  exit=$?"
echo "count-objects AFTER:   $(counts)"
echo "pack dir:              $(ls .git/objects/pack/ | tr '\n' ' ')"
$G cat-file --batch-all-objects --batch-check | sort > $W/objs.after
$G log --all --oneline > $W/log.after
diff -q $W/log.before $W/log.after >/dev/null && echo "git log --all:         与 gc 前一致 ✓" || { echo "git log 变了:"; diff $W/log.before $W/log.after; }
diff -q $W/objs.before $W/objs.after >/dev/null && echo "对象集合/类型/大小:    与 gc 前一致 ✓" || { echo "对象集合变了:"; diff $W/objs.before $W/objs.after; }
echo "git fsck:              exit=$($G fsck --no-progress >/dev/null 2>&1; echo $?)"
echo "git verify-pack:       $($G verify-pack -v .git/objects/pack/*.idx | tail -1)"
echo "pack 名 = sha1(内容): $(
  P=$(ls .git/objects/pack/*.pack)
  SZ=$(stat -c%s "$P")
  NAME=$(basename "$P" .pack)
  SUM=pack-$(head -c $((SZ-20)) "$P" | sha1sum | cut -d' ' -f1)
  [ "$NAME" = "$SUM" ] && echo "match ($NAME) ✓" || echo "MISMATCH: name=$NAME sum=$SUM"
)"
echo "git repack -adf:       exit=$($G repack -adf >/dev/null 2>&1; echo $?)  fsck=$($G fsck --no-progress >/dev/null 2>&1; echo $?)"
echo "mg gc 第二次:          $($MG gc)"
echo "packed-refs:"
cat .git/packed-refs | sed 's/^/    /'
echo "mg fsck after gc:      exit=$($MG fsck; echo $?)"

echo
echo "########## SCENARIO 3: 故意损坏"
echo "--- 3a 改一个 loose 对象的字节"
R=$W/s3a; mkdir -p "$R"; cd "$R"
$G init -q .; printf 'hello\n' > a.txt; $G add -A; $G commit -qm one
OBJ=$(find .git/objects -type f -not -path '*/pack/*' | sort | head -1)
chmod u+w "$OBJ"; printf 'not-a-zlib-stream' > "$OBJ"
echo "corrupted: $(basename "$(dirname "$OBJ")")/$(basename "$OBJ")"
$MG fsck; echo "mg fsck exit=$?  (git fsck exit=$($G fsck --no-progress >/dev/null 2>&1; echo $?))"
echo "--- 3b 截断一个 pack"
R=$W/s3b; mkdir -p "$R"; cd "$R"
$G init -q .; for i in 1 2 3; do printf 'c%s\n' $i > f$i; $G add -A; $G commit -qm c$i; done; $G gc -q
P=$(ls .git/objects/pack/*.pack); SZ=$(stat -c%s "$P"); chmod u+w "$P"; head -c $((SZ-100)) "$P" > t && mv t "$P"
echo "truncated $(basename "$P"): $SZ -> $(stat -c%s "$P") bytes"
$MG fsck; echo "mg fsck exit=$?  (git fsck exit=$($G fsck --no-progress >/dev/null 2>&1; echo $?))"
echo "--- 3c pack 头 count 与 .idx 不一致（重新签名后）"
R=$W/s3c; mkdir -p "$R"; cd "$R"
$G init -q .; for i in 1 2 3; do printf 'c%s\n' $i > f$i; $G add -A; $G commit -qm c$i; done; $G gc -q
P=$(ls .git/objects/pack/*.pack); chmod u+w "$P"
python3 - "$P" <<'PY'
import hashlib, sys
p = sys.argv[1]
b = bytearray(open(p,'rb').read())
b[8:12] = (u32 := int.from_bytes(b[8:12],'big') + 7).to_bytes(4,'big')
b[-20:] = hashlib.sha1(bytes(b[:-20])).digest()
open(p,'wb').write(bytes(b))
PY
$MG fsck; echo "mg fsck exit=$?"
echo "--- 3d index 里的对象被删"
R=$W/s3d; mkdir -p "$R"; cd "$R"
$G init -q .; printf 'a\n' > a; $G add -A; $G commit -qm one; printf 'b\n' > b; $G add -A
OID=$($G ls-files --stage b | awk '{print $2}')
chmod u+w ".git/objects/${OID:0:2}/${OID:2}"; rm ".git/objects/${OID:0:2}/${OID:2}"
$MG fsck; echo "mg fsck exit=$?  (git fsck exit=$($G fsck --no-progress >/dev/null 2>&1; echo $?))"

echo
echo "########## SCENARIO 4: 已经有 git 的 pack 时再 mg gc（多 pack 仓库）"
R=$W/s4; mkdir -p "$R"; cd "$R"
$MG init . >/dev/null; printf 'alpha\n' > a.txt; $MG add . >/dev/null; $MG commit -m first >/dev/null
$G gc -q                       # 真实 git 先打一个 pack
printf 'beta\n' > b.txt; $MG add b.txt >/dev/null; $MG commit -m second >/dev/null   # 新 looose 对象
$MG gc >/dev/null
echo "packs: $(ls .git/objects/pack/*.pack | wc -l) 个"
echo "count-objects: $(counts)"
echo "git fsck: exit=$($G fsck --no-progress >/dev/null 2>&1; echo $?)"
echo "git log --all: $($G log --all --oneline | tr '\n' '|')"
echo "git cat-file HEAD^{tree}: $($G cat-file -t "$($G rev-parse 'HEAD^{tree}')")"
echo "mg fsck: exit=$($MG fsck; echo $?)"

echo
echo "########## SCENARIO 5: bare 仓库上的 mg fsck / mg gc"
R=$W/s5.git; $G init -q --bare "$R"
SEED=$W/s5seed; mkdir -p "$SEED"; cd "$SEED"; $MG init . >/dev/null; printf 'x\n' > x.txt; $MG add . >/dev/null; $MG commit -m x >/dev/null
$G remote add origin "file://$R"; $G push -q origin main; $G push -q origin --tags
cd "$R"
$MG fsck; echo "mg fsck (bare) exit=$?"
$MG gc >/dev/null; echo "mg gc (bare)  exit=$?  packs=$(ls objects/pack/*.pack 2>/dev/null | wc -l)"
$MG fsck; echo "mg fsck after gc exit=$?  git fsck exit=$($G fsck --no-progress >/dev/null 2>&1; echo $?)"
