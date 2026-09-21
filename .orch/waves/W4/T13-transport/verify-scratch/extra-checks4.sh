#!/usr/bin/env bash
# V13 追加检查（第四轮）：两个对照被前一轮的夹具搞错，重做：
#   (11b) 对「引用已打包」的远端做一次**快进** push（走 RefStore::update 的 CAS + packed-refs）。
#   (13b) pull 的「拒绝覆盖本地改动」用正确夹具对拍真实 git（克隆发生在远端前进之前）。
set -uo pipefail

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@e GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@e
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'
export LC_ALL=C

MG=/home/user/Projects/mini-git/target/debug/mg
W=/tmp/v13extra4
rm -rf "$W"; mkdir -p "$W"

echo "############ (11b) 快进 push 到引用已打包的远端"
mkdir -p "$W/11/seed" "$W/11/bare.git"
git -C "$W/11/seed" init -q -b main .
echo a >"$W/11/seed/a.txt"; git -C "$W/11/seed" add -A; git -C "$W/11/seed" commit -qm one
git -C "$W/11" init -q --bare -b main bare.git
git -C "$W/11/seed" push -q "file://$W/11/bare.git" refs/heads/main:refs/heads/main
git -C "$W/11/bare.git" pack-refs --all
echo "packed-refs 内容：$(cat "$W/11/bare.git/packed-refs" | tr '\n' '|')"
echo "loose refs 目录：$(find "$W/11/bare.git/refs" -type f | wc -l) 个文件"
"$MG" clone "file://$W/11/bare.git" "$W/11/work" >/dev/null; echo "clone exit=$?"
echo b >"$W/11/work/b.txt"; git -C "$W/11/work" add -A; git -C "$W/11/work" commit -qm two
"$MG" -C "$W/11/work" push origin main; echo "mg push (FF, packed remote) exit=$?"
echo "bare main=$(git -C "$W/11/bare.git" rev-parse refs/heads/main) work HEAD=$(git -C "$W/11/work" rev-parse HEAD)"
echo "packed-refs 现在：$(cat "$W/11/bare.git/packed-refs" | tr '\n' '|')"
echo "loose refs 目录现在：$(find "$W/11/bare.git/refs" -type f | wc -l) 个文件"
git -C "$W/11/bare.git" fsck --no-progress; echo "bare fsck exit=$?"

echo
echo "############ (13b) pull 拒绝覆盖本地改动（正确夹具：克隆发生在远端前进之前）"
mkdir -p "$W/13/origin"
git -C "$W/13/origin" init -q -b main .
echo one >"$W/13/origin/a.txt"; git -C "$W/13/origin" add -A; git -C "$W/13/origin" commit -qm one
"$MG" clone "file://$W/13/origin" "$W/13/mg-work" >/dev/null
git clone -q "file://$W/13/origin" "$W/13/git-work"
# 远端前进
echo remote-change >"$W/13/origin/a.txt"; git -C "$W/13/origin" add -A; git -C "$W/13/origin" commit -qm remote-edit
# 两边各自弄脏同一个文件
echo local-dirty >"$W/13/mg-work/a.txt"
echo local-dirty >"$W/13/git-work/a.txt"
echo "--- mg pull:"
"$MG" -C "$W/13/mg-work" pull origin; echo "mg pull exit=$?"
echo "mg  a.txt=[$(cat "$W/13/mg-work/a.txt")] HEAD=$(git -C "$W/13/mg-work" rev-parse --short HEAD) status=[$(git -C "$W/13/mg-work" status --porcelain | tr '\n' '|')]"
echo "--- git pull（同一夹具）:"
git -C "$W/13/git-work" pull --no-rebase origin main; echo "git pull exit=$?"
echo "git a.txt=[$(cat "$W/13/git-work/a.txt")] HEAD=$(git -C "$W/13/git-work" rev-parse --short HEAD) status=[$(git -C "$W/13/git-work" status --porcelain | tr '\n' '|')]"
