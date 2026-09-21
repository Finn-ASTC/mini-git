#!/usr/bin/env bash
set -uo pipefail
G="git -c init.defaultBranch=main -c user.name=A -c user.email=a@example.com -c gc.auto=0 -c commit.gpgsign=false"
echo "=== packed-refs exact bytes (cat -A) of git pack-refs --all"
R=/tmp/t15/p3; rm -rf $R; mkdir $R; cd $R; $G init -q .
printf 'x\n' > x; $G add -A; $G commit -qm one; $G tag -a v1 -m tag; $G tag light
git pack-refs --all
cat -A .git/packed-refs
echo "=== fresh repo fsck (unborn HEAD)"
F=/tmp/t15/p3fresh; rm -rf $F; mkdir $F; cd $F; $G init -q .
$G fsck --no-progress; echo "exit=$?"
$G fsck --full --no-progress; echo "full exit=$?"
echo "=== repo with dangling object: fsck exit + output"
D=/tmp/t15/p3dangle; rm -rf $D; mkdir $D; cd $D; $G init -q .
printf 'a\n' > a; $G add -A; $G commit -qm one
printf 'dangling content\n' | $G hash-object -w --stdin >/dev/null
$G fsck --no-progress; echo "exit=$?"
echo "=== repo whose index references a missing object"
I=/tmp/t15/p3index; rm -rf $I; mkdir $I; cd $I; $G init -q .
printf 'a\n' > a; $G add -A; $G commit -qm one
printf 'b\n' > b; $G add -A
# delete the blob for b from the object database
OID=$($G ls-files --stage b | awk '{print $2}')
echo "blob for b = $OID"
chmod u+w ".git/objects/${OID:0:2}/${OID:2}" && rm ".git/objects/${OID:0:2}/${OID:2}"
$G fsck --no-progress 2>&1 | sed 's/^/   /'; echo "exit=$?"
echo "=== index trailer corrupted"
J=/tmp/t15/p3idxsum; rm -rf $J; mkdir $J; cd $J; $G init -q .
printf 'a\n' > a; $G add -A; $G commit -qm one
printf 'b\n' > b; $G add -A
chmod u+w .git/index; printf 'X' | dd of=.git/index bs=1 seek=100 conv=notrunc status=none
$G fsck --no-progress 2>&1 | sed 's/^/   /'; echo "exit=$?"
$G status --porcelain 2>&1 | sed 's/^/   /'; echo "status exit=$?"
