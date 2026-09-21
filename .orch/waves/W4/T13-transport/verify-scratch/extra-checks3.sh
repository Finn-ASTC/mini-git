#!/usr/bin/env bash
# V13 追加检查（第三轮）：打包过的远端（packed objects / packed-refs）、
# 「真实 git 克隆 mg 写的仓库并从里面 push」、本地 gc 后的 fetch。
set -uo pipefail

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@e GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@e
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'
export LC_ALL=C

MG=/home/user/Projects/mini-git/target/debug/mg
W=/tmp/v13extra3
rm -rf "$W"; mkdir -p "$W"

mk_origin() { # $1 = dir
  mkdir -p "$1"
  git -C "$1" init -q -b main .
  echo one >"$1/a.txt"
  mkdir -p "$1/sub"
  echo beta >"$1/sub/b.txt"
  git -C "$1" add -A
  git -C "$1" commit -qm one
  echo two >>"$1/a.txt"
  git -C "$1" add -A
  git -C "$1" commit -qm two
  git -C "$1" branch feature
  git -C "$1" tag light
}

echo "############ (10) 远端被 git gc 打包（无 loose 对象 + packed-refs）后 clone"
mk_origin "$W/10/origin"
git -C "$W/10/origin" gc -q --aggressive --prune=now
git -C "$W/10/origin" pack-refs --all
echo "loose objects left: $(find "$W/10/origin/.git/objects" -type f -name '*' -not -path '*/pack/*' | wc -l)"
echo "packed-refs exists? $(test -f "$W/10/origin/.git/packed-refs" && echo yes || echo no)"
"$MG" clone "file://$W/10/origin" "$W/10/mg-copy" >"$W/10/mg.out" 2>&1; echo "mg clone exit=$? out=$(cat "$W/10/mg.out")"
git clone -q "file://$W/10/origin" "$W/10/git-copy"
diff <(git -C "$W/10/mg-copy" show-ref | sort) <(git -C "$W/10/git-copy" show-ref | sort) && echo "show-ref IDENTICAL"
diff <(git -C "$W/10/mg-copy" log --oneline) <(git -C "$W/10/origin" log --oneline) && echo "log IDENTICAL"
echo "mg status=[$(git -C "$W/10/mg-copy" status --porcelain)]  sub/b.txt=[$(cat "$W/10/mg-copy/sub/b.txt")]"
git -C "$W/10/mg-copy" fsck --no-progress; echo "fsck exit=$?"

echo
echo "############ (11) 真实 git 克隆 mg 写的仓库，并从那个克隆 push 回裸仓库"
git -C "$W/10" init -q --bare -b main bare.git
"$MG" -C "$W/10/mg-copy" push "file://$W/10/bare.git" main; echo "mg push exit=$?"
echo "bare main: $(git -C "$W/10/bare.git" rev-parse refs/heads/main) vs mg-copy HEAD $(git -C "$W/10/mg-copy" rev-parse HEAD)"
git clone "file://$W/10/mg-copy" "$W/11/git-clone-of-mg" 2>&1 | tail -2; echo "git clone(from mg) exit=${PIPESTATUS[0]}"
git -C "$W/11/git-clone-of-mg" fsck --no-progress; echo "fsck exit=$?"
echo three >"$W/11/git-clone-of-mg/c.txt"; git -C "$W/11/git-clone-of-mg" add -A; git -C "$W/11/git-clone-of-mg" commit -qm three
git -C "$W/11/git-clone-of-mg" push "file://$W/10/bare.git" main 2>&1 | tail -2; echo "git push (into the bare mg pushed to) exit=${PIPESTATUS[0]}"
echo "bare main now: $(git -C "$W/10/bare.git" rev-parse refs/heads/main)"
"$MG" -C "$W/10/mg-copy" fetch "file://$W/10/bare.git"; echo "mg fetch after git push exit=$?"
echo "mg fetch 后 remote ref: $(git -C "$W/10/mg-copy" show-ref | sort | tr '\n' '|')"
# mg push 到「引用已打包」的远端
git -C "$W/10/bare.git" pack-refs --all
echo "四" >"$W/10/mg-copy/d.txt"; git -C "$W/10/mg-copy" add -A; git -C "$W/10/mg-copy" commit -qm four
"$MG" -C "$W/10/mg-copy" push "file://$W/10/bare.git" main; echo "mg push to packed-refs remote exit=$?"
echo "bare main after: $(git -C "$W/10/bare.git" rev-parse refs/heads/main) == mg-copy HEAD $(git -C "$W/10/mg-copy" rev-parse HEAD)"
git -C "$W/10/bare.git" fsck --no-progress; echo "bare fsck exit=$?"

echo
echo "############ (12) 本地 gc 打包后 mg fetch（PackSet 路径）"
mk_origin "$W/12/origin"
"$MG" clone "file://$W/12/origin" "$W/12/work" >/dev/null
git -C "$W/12/work" gc -q --prune=now
echo "loose objects left in work: $(find "$W/12/work/.git/objects" -type f -not -path '*/pack/*' | wc -l)"
echo two >>"$W/12/origin/a.txt"; git -C "$W/12/origin" add -A; git -C "$W/12/origin" commit -qm three
"$MG" -C "$W/12/work" pull origin; echo "mg pull exit=$?"
echo "work HEAD=$(git -C "$W/12/work" rev-parse HEAD) origin main=$(git -C "$W/12/origin" rev-parse refs/heads/main)"
echo "status=[$(git -C "$W/12/work" status --porcelain)]"; git -C "$W/12/work" fsck --no-progress; echo "fsck exit=$?"

echo
echo "############ (13) pull 遇到会被覆盖的本地改动（作者声称拒绝）"
mk_origin "$W/13/origin"
"$MG" clone "file://$W/13/origin" "$W/13/work" >/dev/null
echo remote-change >"$W/13/origin/a.txt"; git -C "$W/13/origin" add -A; git -C "$W/13/origin" commit -qm remote-edit
echo local-dirty >"$W/13/work/a.txt"
"$MG" -C "$W/13/work" pull origin; echo "mg pull exit=$?"
echo "work a.txt=[$(cat "$W/13/work/a.txt")]  origin a.txt=[$(cat "$W/13/origin/a.txt")]"
echo "--- 对照真实 git："
git clone -q "file://$W/13/origin" "$W/13/git-work" 2>/dev/null
echo local-dirty >"$W/13/git-work/a.txt"
git -C "$W/13/git-work" pull --no-rebase origin main 2>&1 | tail -2; echo "git pull exit=${PIPESTATUS[0]}"
