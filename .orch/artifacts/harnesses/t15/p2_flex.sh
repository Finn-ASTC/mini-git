#!/usr/bin/env bash
# Probe 2: flexibility of real git w.r.t. pack names, packed-refs peeling claims, corruption exit codes.
set -uo pipefail
G="git -c init.defaultBranch=main -c user.name=A -c user.email=a@example.com -c gc.auto=0 -c commit.gpgsign=false"
R=/tmp/t15/p2
rm -rf "$R"; mkdir -p "$R"; cd "$R"
$G init -q .
for i in 1 2 3; do printf 'content %s\n' "$i" > "f$i.txt"; $G add -A; $G commit -qm "c$i"; done
$G tag -a v1 -m 'annotated tag'
$G gc -q
P=$(ls .git/objects/pack/*.pack)
echo "=== A. rename pack to a bogus name; does git fsck / verify-pack / cat-file care?"
NEW=.git/objects/pack/pack-0000000000000000000000000000000000000000
cp "$P" "$NEW.pack"; cp "${P%.pack}.idx" "$NEW.idx"; rm -f "$P" "${P%.pack}.idx" "${P%.pack}.rev"
echo "-- count-objects:"; $G count-objects -v | sed -n '3,5p'
echo "-- fsck:"; $G fsck --no-progress 2>&1 | sed 's/^/   /'; echo "   fsck exit=$?"
echo "-- verify-pack on bogus name:"; $G verify-pack -v "$NEW.idx" 2>&1 | tail -2
echo "-- cat-file of a packed blob (git rev-list --objects --all | head):"
OID=$($G cat-file --batch-all-objects --batch-check='%(objectname) %(objecttype)' | awk '$2=="blob"{print $1; exit}')
$G cat-file -p "$OID" | sed 's/^/   /'
echo
echo "=== B. packed-refs claiming fully-peeled but with the ^ line removed"
RH=/tmp/t15/p2refs; rm -rf $RH; mkdir $RH; cd $RH; $G init -q .
printf 'x\n' > x; $G add -A; $G commit -qm one; $G tag -a v1 -m tag; $G tag light
$G pack-refs --all
echo "-- git's packed-refs:"; cat .git/packed-refs
grep -v '^\^' .git/packed-refs > .git/packed-refs.tmp && mv .git/packed-refs.tmp .git/packed-refs
echo "-- after removing ^ line:"; cat .git/packed-refs
echo "-- show-ref exit=$?"; $G show-ref; echo "   exit=$?"
echo "-- rev-parse v1^{commit} = $($G rev-parse 'v1^{commit}')"
echo "-- fsck:"; $G fsck --no-progress 2>&1 | sed 's/^/   /'; echo "   fsck exit=$?"
cd "$R"
echo
echo "=== C. corrupt loose object (chmod first)"
CH=/tmp/t15/p2corrupt; rm -rf $CH; mkdir $CH; cd $CH; $G init -q .
printf 'hello\n' > a.txt; $G add -A; $G commit -qm one
OBJ=$(find .git/objects -type f -not -path '*/pack/*' | sort | head -1)
chmod u+w "$OBJ"; printf 'garbage-garbage' > "$OBJ"
$G cat-file --batch-all-objects --batch-check 2>&1 | sed 's/^/   /' | head -5
$G fsck --no-progress 2>&1 | sed 's/^/   /'; echo "   fsck exit=$?"
echo
echo "=== D. truncated pack"
TR=/tmp/t15/p2trunc; rm -rf $TR; mkdir $TR; cd $TR; $G init -q .
for i in 1 2 3; do printf 'c %s\n' $i > f$i; $G add -A; $G commit -qm c$i; done
$G gc -q
P=$(ls .git/objects/pack/*.pack); SZ=$(stat -c%s "$P")
head -c $((SZ - 100)) "$P" > t.pack; mv t.pack "$P"
$G fsck --no-progress 2>&1 | sed 's/^/   /'; echo "   fsck exit=$?"
$G verify-pack -v "${P%.pack}.idx" 2>&1 | tail -3 | sed 's/^/   /'; echo "   verify-pack exit=$?"
