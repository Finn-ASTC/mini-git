#!/usr/bin/env bash
# Scenario 1-3 for T15: manual, raw-output evidence.
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c init.defaultBranch=main -c user.name=A -c user.email=a@example.com -c gc.auto=0 -c commit.gpgsign=false -c protocol.file.allow=always"
W=/tmp/t15/verify
rm -rf "$W"; mkdir -p "$W"

echo "################ SCENARIO 1: real git repo incl. packfile -> mg fsck"
R=$W/s1; mkdir -p "$R"; cd "$R"
$G init -q .
mkdir -p sub/deep
printf 'one\n' > a.txt; printf 'two\n' > sub/b.txt; printf 'three\n' > sub/deep/c.txt
$G add -A; $G commit -qm 'c1'
printf 'one changed\n' > a.txt; $G add -A; $G commit -qm 'c2'
$G tag -a v1 -m 'annotated'
$G tag light
$G branch feature
printf 'extra\n' > 'weird name.txt'; $G add -A; $G commit -qm 'c3 weird name'
$G gc -q
echo "-- packs:"; ls .git/objects/pack/
echo "-- mg fsck:"; $MG fsck; echo "   exit=$?"
echo "-- mg fsck --full:"; $MG fsck --full; echo "   exit=$?"
echo "-- git fsck for comparison:"; $G fsck --no-progress; echo "   exit=$?"
echo "-- a repo with a dangling object:"
printf 'dangling\n' | $G hash-object -w --stdin > /dev/null
$MG fsck; echo "   mg fsck exit=$? (dangling must NOT be an error)"

echo
echo "################ SCENARIO 2: mg gc on a loose-only repo"
R=$W/s2; mkdir -p "$R"; cd "$R"
$MG init .
printf 'alpha\n' > a.txt; mkdir dir; printf 'beta\n' > dir/b.txt
$MG add .
$MG commit -m 'first'
printf 'alpha v2\n' > a.txt; $MG add a.txt; $MG commit -m 'second'
$MG tag v1
$MG tag -a va -m 'annotated tag'
$MG branch side
echo "-- mg status --porcelain: [$($MG status --porcelain)]"
echo "-- git count-objects BEFORE:"; $G count-objects -v
echo "-- git log --all --oneline BEFORE:"; $G log --all --oneline | tee /tmp/t15/verify/log.before
echo "-- git show-ref BEFORE:"; $G show-ref | tee /tmp/t15/verify/refs.before
echo "-- objects dump BEFORE:"; $G cat-file --batch-all-objects --batch-check | sort | tee /tmp/t15/verify/objs.before
echo "-- mg fsck before gc:"; $MG fsck; echo "   exit=$?"
echo "-- mg gc:"; $MG gc; echo "   exit=$?"
echo "-- pack dir:"; ls -l .git/objects/pack/
echo "-- git count-objects AFTER:"; $G count-objects -v
echo "-- git log --all --oneline AFTER:"; $G log --all --oneline | tee /tmp/t15/verify/log.after
echo "-- diff log:"; diff /tmp/t15/verify/log.before /tmp/t15/verify/log.after && echo "   log identical"
echo "-- git show-ref AFTER:"; $G show-ref; diff <($G show-ref) /tmp/t15/verify/refs.before && echo "   refs identical"
echo "-- objects dump AFTER:"; $G cat-file --batch-all-objects --batch-check | sort > /tmp/t15/verify/objs.after
diff /tmp/t15/verify/objs.before /tmp/t15/verify/objs.after && echo "   object set identical"
echo "-- git cat-file spot checks:"
for oid in $($G rev-list --objects --all | awk '{print $1}' | head -3); do
  echo "   $oid: $($G cat-file -t "$oid") $($G cat-file -s "$oid") bytes"
done
echo "-- git fsck after mg gc:"; $G fsck --no-progress; echo "   exit=$?"
echo "-- git verify-pack -v:"; $G verify-pack -v .git/objects/pack/*.idx | tail -3
echo "-- git repack -adf (does git accept our pack?):"; $G repack -adf 2>&1 | sed 's/^/   /'; echo "   exit=$?"
$G fsck --no-progress; echo "   fsck after repack exit=$?"
echo "-- mg gc idempotence (second run):"; $MG gc; echo "   exit=$?"
ls .git/objects/pack/
echo "-- packed-refs after mg gc:"; cat -A .git/packed-refs
echo "-- compare with git pack-refs --all output (fresh copy):"
CP=$W/s2copy; rm -rf "$CP"; cp -r "$R" "$CP"; cd "$CP"; $G pack-refs --all
cat -A .git/packed-refs
if diff <(sed 's/[[:space:]]*$//' "$R/.git/packed-refs") <(sed 's/[[:space:]]*$//' "$CP/.git/packed-refs") >/dev/null; then
  echo "   packed-refs identical to git's (ignoring trailing whitespace)"
else
  echo "   DIFFERENCE:"; diff "$R/.git/packed-refs" "$CP/.git/packed-refs"
fi
diff "$R/.git/packed-refs" "$CP/.git/packed-refs" >/dev/null && echo "   byte-identical" || echo "   (byte diff above)"

echo
echo "################ SCENARIO 3a: corrupted loose object"
R=$W/s3a; mkdir -p "$R"; cd "$R"
$G init -q .; printf 'hello\n' > a.txt; $G add -A; $G commit -qm one
OBJ=$(find .git/objects -type f -not -path '*/pack/*' | sort | head -1)
echo "-- corrupting $OBJ"; chmod u+w "$OBJ"; printf 'not-zlib-at-all' > "$OBJ"
$MG fsck; echo "   mg fsck exit=$?"
$MG fsck --full; echo "   mg fsck --full exit=$?"

echo
echo "################ SCENARIO 3b: truncated pack"
R=$W/s3b; mkdir -p "$R"; cd "$R"
$G init -q .; for i in 1 2 3; do printf 'c %s\n' $i > f$i; $G add -A; $G commit -qm c$i; done; $G gc -q
P=$(ls .git/objects/pack/*.pack); SZ=$(stat -c%s "$P")
echo "-- truncating $P from $SZ bytes"; head -c $((SZ - 100)) "$P" > t.pack && mv t.pack "$P"
$MG fsck; echo "   mg fsck exit=$?"

echo
echo "################ SCENARIO 3c: idx/pack object-count mismatch + idx-only"
R=$W/s3c; mkdir -p "$R"; cd "$R"
$G init -q .; for i in 1 2 3; do printf 'c %s\n' $i > f$i; $G add -A; $G commit -qm c$i; done; $G gc -q
IDX=$(ls .git/objects/pack/*.idx); PK=${IDX%.idx}.pack
echo "-- deleting the .pack of $IDX"; rm "$PK"
$MG fsck; echo "   mg fsck exit=$?"

echo
echo "################ SCENARIO 3d: index referencing a missing object"
R=$W/s3d; mkdir -p "$R"; cd "$R"
$G init -q .; printf 'a\n' > a; $G add -A; $G commit -qm one
printf 'b\n' > b; $G add -A
OID=$($G ls-files --stage b | awk '{print $2}')
chmod u+w ".git/objects/${OID:0:2}/${OID:2}" && rm ".git/objects/${OID:0:2}/${OID:2}"
echo "-- removed blob $OID"
$MG fsck; echo "   mg fsck exit=$?"

echo
echo "################ SCENARIO 3e: broken index trailer"
R=$W/s3e; mkdir -p "$R"; cd "$R"
$G init -q .; printf 'a\n' > a; $G add -A; $G commit -qm one
chmod u+w .git/index; printf 'ZZZZ' | dd of=.git/index bs=1 seek=60 conv=notrunc status=none
$MG fsck; echo "   mg fsck exit=$?"

echo
echo "################ SCENARIO 3f: mg gc refuses to pack a corrupt loose object"
R=$W/s3f; mkdir -p "$R"; cd "$R"
$MG init .; printf 'x\n' > x; $MG add .; $MG commit -m one
OBJ=$(find .git/objects -type f -not -path '*/pack/*' | sort | head -1)
printf 'garbage' > "$OBJ.fix" && mv "$OBJ.fix" "$OBJ"
echo "-- corrupting $OBJ"; $MG gc; echo "   mg gc exit=$? (must be non-zero)"
echo "-- loose objects still present: $(find .git/objects -type f -not -path '*/pack/*' | wc -l)"

echo
echo "################ SCENARIO 4: mg fsck on an mg-created repo"
R=$W/s4; mkdir -p "$R"; cd "$R"
$MG init .; printf 'q\n' > q.txt; $MG add .; $MG commit -m 'q'
$MG fsck; echo "   mg fsck exit=$? (clean mg repo)"
$MG gc >/dev/null; $MG fsck; echo "   mg fsck exit=$? (after gc)"
$G fsck --no-progress; echo "   git fsck exit=$? (after mg gc)"
echo "-- mg-created repo with unborn HEAD:"
R=$W/s5; mkdir -p "$R"; cd "$R"; $MG init .; $MG fsck; echo "   mg fsck exit=$? (unborn HEAD)"
$G fsck --no-progress; echo "   git fsck exit=$?"
