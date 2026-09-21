#!/usr/bin/env bash
# Probe 1: how does real git derive pack file names? exit codes of fsck? packed-refs format?
set -uo pipefail
G="git -c init.defaultBranch=main -c user.name=A -c user.email=a@example.com -c gc.auto=0 -c commit.gpgsign=false"
R=/tmp/t15/p1
rm -rf "$R"; mkdir -p "$R"; cd "$R"
$G init -q .
for i in 1 2 3; do printf 'content %s\n' "$i" > "f$i.txt"; $G add -A; $G commit -qm "c$i"; done
$G tag -a v1 -m 'annotated tag'
$G tag lightweight
echo "--- ls-files loose object count before gc"
find .git/objects -type f -not -path '*/pack/*' | wc -l
echo "--- git count-objects before"
$G count-objects -v
$G gc -q 2>&1 | sed 's/^/gc: /'
echo "--- pack dir listing"
ls -l .git/objects/pack/
P=$(ls .git/objects/pack/*.pack)
echo "pack=$P"
echo "name-part: $(basename "$P" .pack)"
echo "sha1 whole file     : $(sha1sum "$P" | cut -d' ' -f1)"
echo "sha1 minus trailer  : $(head -c $(( $(stat -c%s "$P") - 20 )) "$P" | sha1sum | cut -d' ' -f1)"
echo "sha1 of .idx whole  : $(sha1sum "${P%.pack}.idx" | cut -d' ' -f1)"
echo "--- verify-pack"
$G verify-pack -v "${P%.pack}.idx" | tail -5
echo "--- packed-refs content"
cat .git/packed-refs
echo "--- count-objects after"
$G count-objects -v
echo "--- fsck exit code (clean)"
$G fsck --no-progress; echo "fsck exit=$?"
echo "--- fsck with a corrupt loose object"
$G init -q /tmp/t15/p1corrupt 2>/dev/null
cd /tmp/t15/p1corrupt
printf 'hello\n' > a.txt; $G add -A; $G commit -qm one
OBJ=$(find .git/objects -type f -not -path '*/pack/*' | sort | head -1)
echo "corrupting $OBJ"
printf 'garbage-garbage' > "$OBJ"
$G fsck --no-progress; echo "fsck exit=$?"
cd "$R"
