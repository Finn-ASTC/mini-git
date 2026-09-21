#!/usr/bin/env bash
# mg clone file:// — does it work against an mg origin? a git origin? with tags?
set -uo pipefail
MG=/home/user/Projects/mini-git/target/debug/mg
G="git -c gc.auto=0 -c init.defaultBranch=main -c user.name=A -c user.email=a@e.com -c protocol.file.allow=always"
W=/tmp/t15/clone; rm -rf "$W"; mkdir -p "$W"

echo "### A. origin created by real git (no tags)"
O=$W/gitorigin; mkdir -p "$O"; cd "$O"; $G init -q .; printf 'alpha\n' > a.txt; $G add .; $G commit -qm one
cd "$W"; $MG clone "file://$O" $W/cloneA; echo "   exit=$?"
[ -d $W/cloneA/.git ] && { cd $W/cloneA; echo "   git fsck: $($G fsck --no-progress 2>&1|tr '\n' '|')"; $G log --oneline | sed 's/^/   /'; echo "   status: [$($G status --porcelain|tr '\n' '|')]"; }

echo "### B. origin created by mg (no tags)"
O=$W/mgorigin; mkdir -p "$O"; cd "$O"; $MG init . >/dev/null; printf 'alpha\n' > a.txt; $MG add . >/dev/null; $MG commit -m one >/dev/null
cd "$W"; $MG clone "file://$O" $W/cloneB; echo "   exit=$?"

echo "### C. git origin WITH a tag"
O=$W/gitorigin2; mkdir -p "$O"; cd "$O"; $G init -q .; printf 'alpha\n' > a.txt; $G add .; $G commit -qm one; $G tag v1
cd "$W"; $MG clone "file://$O" $W/cloneC; echo "   exit=$?"
[ -d $W/cloneC/.git ] && { cd $W/cloneC; $G show-ref | sed 's/^/   /'; }

echo "### D. git clone of an mg-created origin (control: is the origin sane?)"
O=$W/mgorigin; cd "$W"; $G clone "file://$O" $W/gitcloneD >/dev/null 2>&1; echo "   git clone exit=$?"
[ -d $W/gitcloneD ] && { cd $W/gitcloneD; $G log --oneline | sed 's/^/   /'; }

echo "### E. mg clone with verbose stderr"
O=$W/mgorigin; cd "$W"; $MG clone "file://$O" $W/cloneE; echo "   exit=$?"
echo "### F. mg clone from a *bare* git origin (git init --bare + push)"
O=$W/bare.git; $G init -q --bare "$O"; cd "$W/gitorigin"; $G remote add b "$O"; $G push -q b main 2>&1|sed 's/^/   /'
cd "$W"; $MG clone "file://$O" $W/cloneF; echo "   exit=$?"
