#!/usr/bin/env bash
# Pin down mg push's refusal: local HEAD vs remote bare-ness. Compare with real git.
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c gc.auto=0 -c init.defaultBranch=main -c user.name=A -c user.email=a@e.com -c protocol.file.allow=always"
W=/tmp/t15/push; rm -rf "$W"; mkdir -p "$W"

mkseed() { # mkseed <dir>
  local d="$1"; mkdir -p "$d"; cd "$d"
  $MG init . >/dev/null; printf 'alpha\n' > a.txt
  $MG add . >/dev/null; $MG commit -m first >/dev/null; $MG tag v1 >/dev/null
}

echo "### 1. mg push main -> bare origin, while local HEAD is on main"
mkseed $W/s1; BARE=$W/b1.git; $G init -q --bare "$BARE"
$G -C $W/s1 remote add origin "file://$BARE"; $G -C $W/s1 push -q origin main; $G -C $W/s1 push -q origin --tags
cd $W/s1; printf 'x\n' > x.txt; $MG add x.txt >/dev/null; $MG commit -m second >/dev/null
$MG push origin main; echo "   mg exit=$?  bare main=$($G -C $BARE rev-parse refs/heads/main) local HEAD=$($G rev-parse HEAD)"

echo "### 1b. same scenario with mg clone as the client (HEAD on main)"
cd $W; $MG clone "file://$BARE" $W/k1 >/dev/null 2>&1; cd $W/k1
printf 'y\n' > y.txt; $MG add y.txt >/dev/null; $MG commit -m third >/dev/null
$MG push origin main; echo "   mg exit=$?"

echo "### 2. real git pushes main to the same bare origin (control)"
cd $W/s1
$G push origin main 2>&1 | sed 's/^/   git: /'; echo "   git push exit=$?"
echo "   bare main=$($G -C $BARE rev-parse refs/heads/main) local HEAD=$($G rev-parse HEAD)"

echo "### 3. mg push of a branch that is NOT checked out locally"
cd $W/s1; $G branch side; $MG switch side >/dev/null 2>&1 || $G checkout -q side
printf 'z\n' > z.txt; $MG add z.txt >/dev/null; $MG commit -m 'side work' >/dev/null
$MG push origin side; echo "   mg exit=$?  bare side=$($G -C $BARE rev-parse refs/heads/side 2>&1)"

echo "### 4. mg push to a NON-bare origin whose branch is checked out (git refuses too)"
NB=$W/nb; mkdir -p $NB; cd $NB; $G init -q .; printf 'q\n' > q.txt; $G add .; $G commit -qm q
cd $W/s1; $G remote add nb "file://$NB"; $G push -q nb main 2>&1 | sed 's/^/   git: /'
$MG push nb main; echo "   mg exit=$? (expected: refused, matches git)"
$G push nb main 2>&1 | sed 's/^/   git control: /'; echo "   git exit=$?"

echo "### 5. mg push --force / -u behaviour"
cd $W/k1; $MG push -u origin main; echo "   mg push -u exit=$?"
