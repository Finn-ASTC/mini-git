#!/usr/bin/env bash
# Bare origin workflow: mg clone -> mg push -> second clone -> mg fetch/pull
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c gc.auto=0 -c init.defaultBranch=main -c user.name=A -c user.email=a@e.com -c protocol.file.allow=always"
W=/tmp/t15/rt2; rm -rf "$W"; mkdir -p "$W"

SEED=$W/seed; mkdir -p "$SEED"; cd "$SEED"
$MG init . >/dev/null; printf 'alpha\n' > a.txt; mkdir dir; printf 'beta\n' > dir/b.txt
$MG add . >/dev/null; $MG commit -m first >/dev/null; $MG tag v1 >/dev/null

BARE=$W/origin.git
$G init -q --bare "$BARE"
$G -C "$SEED" remote add origin "file://$BARE"
$G -C "$SEED" push -q origin main; $G -C "$SEED" push -q origin --tags
echo "-- bare refs: $($G -C "$BARE" show-ref | tr '\n' '|')"

cd $W; $MG clone "file://$BARE" $W/c1; echo "clone c1 exit=$?"
cd $W/c1; echo "-- c1 refs: $($G show-ref|tr '\n' '|')"; echo "-- c1 files: $(find . -type f -not -path './.git/*'|sort|tr '\n' ' ')"
printf 'gamma\n' > c.txt; $MG add c.txt >/dev/null; $MG commit -m 'from c1' >/dev/null
echo "-- mg push origin main:"; $MG push origin main; echo "   exit=$?"
echo "-- bare main: $($G -C "$BARE" rev-parse refs/heads/main) / c1 HEAD: $($G rev-parse HEAD)"
echo "-- bare fsck: $($G -C "$BARE" fsck --no-progress 2>&1|tr '\n' '|')"

cd $W; $MG clone "file://$BARE" $W/c2; echo "clone c2 exit=$?"
cd $W/c2; printf 'delta\n' > d.txt; $MG add d.txt >/dev/null; $MG commit -m 'from c2' >/dev/null
$MG push origin main >/dev/null 2>&1; echo "-- c2 push exit=$?"

cd $W/c1
echo "-- mg fetch origin:"; $MG fetch origin; echo "   exit=$?"
echo "-- refs/remotes/origin/main: $($G rev-parse refs/remotes/origin/main) vs bare: $($G -C "$BARE" rev-parse refs/heads/main)"
echo "-- mg pull origin:"; $MG pull origin; echo "   exit=$?"
echo "-- c1 log: $($G log --oneline|tr '\n' '|') vs bare: $($G -C "$BARE" log --oneline|tr '\n' '|')"
echo "-- c1 status: [$($G status --porcelain|tr '\n' '|')] fsck: $($G fsck --no-progress 2>&1|tr '\n' '|')"
echo "-- c1 files: $(find . -type f -not -path './.git/*'|sort|tr '\n' ' ')"
