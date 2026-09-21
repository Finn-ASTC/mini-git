#!/usr/bin/env bash
# Round trip with a TAGGED origin: clone -> push -> fetch -> pull
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c gc.auto=0 -c init.defaultBranch=main -c user.name=A -c user.email=a@e.com -c protocol.file.allow=always"
W=/tmp/t15/rt; rm -rf "$W"; mkdir -p "$W"

O=$W/origin; mkdir -p "$O"; cd "$O"
$MG init . >/dev/null
printf 'alpha\n' > a.txt; mkdir dir; printf 'beta\n' > dir/b.txt
$MG add . >/dev/null; $MG commit -m first >/dev/null; $MG tag v1 >/dev/null
echo "-- origin refs: $($G show-ref | tr '\n' '|')"

cd "$W"; $MG clone "file://$O" $W/copy; echo "clone exit=$?"
cd $W/copy
echo "-- clone refs: $($G show-ref | tr '\n' '|')"
echo "-- clone status: [$($G status --porcelain | tr '\n' '|')]"
echo "-- clone log: $($G log --oneline | tr '\n' '|')"
echo "-- clone fsck: $($G fsck --no-progress 2>&1 | tr '\n' '|')"
echo "-- remotes config: $(cat .git/config | tr '\n' '|')"
echo "-- files: $(find . -type f -not -path './.git/*' | sort | tr '\n' ' ')"

printf 'gamma\n' > c.txt; $MG add c.txt >/dev/null; $MG commit -m 'from the clone' >/dev/null
echo "-- push:"; $MG push origin main; echo "   push exit=$?"
echo "-- origin main: $($G -C "$O" rev-parse refs/heads/main) / clone HEAD: $($G rev-parse HEAD)"
echo "-- origin fsck: $($G -C "$O" fsck --no-progress 2>&1 | tr '\n' '|')"

cd "$O"; printf 'delta\n' > d.txt; $MG add d.txt >/dev/null; $MG commit -m 'on the origin' >/dev/null
cd $W/copy
echo "-- fetch:"; $MG fetch origin; echo "   fetch exit=$?"
echo "-- refs/remotes/origin/main: $($G rev-parse refs/remotes/origin/main 2>&1) vs origin main: $($G -C "$O" rev-parse refs/heads/main)"
echo "-- pull:"; $MG pull origin; echo "   pull exit=$?"
echo "-- clone log: $($G log --oneline | tr '\n' '|')"
echo "-- origin log: $($G -C "$O" log --oneline | tr '\n' '|')"
echo "-- clone status: [$($G status --porcelain | tr '\n' '|')]  fsck: $($G fsck --no-progress 2>&1 | tr '\n' '|')"
echo "-- files: $(find . -type f -not -path './.git/*' | sort | tr '\n' ' ')"
