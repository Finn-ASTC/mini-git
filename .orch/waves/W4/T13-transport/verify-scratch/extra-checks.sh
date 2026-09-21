#!/usr/bin/env bash
# V13 追加独立检查（不写共享工作树，全部在 /tmp/v13extra 下）。
# 覆盖：远端 detached HEAD / 无 tag / unborn HEAD 的 clone 对拍、push -u 的 config、
#       non-bare 远端「当前分支」拒绝、pull 的非快进路径、clone 默认目录名。
set -uo pipefail

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@e GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@e
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'
export LC_ALL=C

MG=/home/user/Projects/mini-git/target/debug/mg
W=/tmp/v13extra
rm -rf "$W"
mkdir -p "$W"

echo "############ mg binary: $MG"
ls -l "$MG"

echo
echo "############ (1) 远端无 tag：mg clone vs git clone"
mkdir -p "$W/1/origin"
git -C "$W/1/origin" init -q -b main .
echo a >"$W/1/origin/a.txt"
git -C "$W/1/origin" add -A
git -C "$W/1/origin" commit -qm one
"$MG" clone "file://$W/1/origin" "$W/1/mg-copy" >"$W/1/mg.out" 2>"$W/1/mg.err"
echo "mg clone exit=$?  stdout=$(cat "$W/1/mg.out")  stderr=$(cat "$W/1/mg.err")"
git clone -q "file://$W/1/origin" "$W/1/git-copy" 2>"$W/1/git.err"
echo "git clone exit=$?  stderr=$(cat "$W/1/git.err")"
echo "--- show-ref diff (mg vs git):"
diff <(git -C "$W/1/mg-copy" show-ref | sort) <(git -C "$W/1/git-copy" show-ref | sort) && echo "show-ref IDENTICAL"
echo "--- refs/tags in both (for-each-ref):"
git -C "$W/1/mg-copy" for-each-ref --format='mg  %(refname)' 
git -C "$W/1/git-copy" for-each-ref --format='git %(refname)'
echo "--- fsck mg clone:"; git -C "$W/1/mg-copy" fsck --no-progress; echo "fsck exit=$?"
echo "--- status mg clone: [$(git -C "$W/1/mg-copy" status --porcelain)]"

echo
echo "############ (2) 远端 detached HEAD（广告里没有 symref）：mg clone vs git clone"
mkdir -p "$W/2/origin"
git -C "$W/2/origin" init -q -b main .
echo a >"$W/2/origin/a.txt"; git -C "$W/2/origin" add -A; git -C "$W/2/origin" commit -qm one
echo b >"$W/2/origin/b.txt"; git -C "$W/2/origin" add -A; git -C "$W/2/origin" commit -qm two
git -C "$W/2/origin" checkout -q --detach HEAD
echo "origin HEAD: $(cat "$W/2/origin/.git/HEAD")"
echo "advertise has symref? $(git -C "$W/2/origin" -c protocol.version=0 upload-pack --advertise-refs . | tr '\0' '\n' | tr -d '\000' | head -c 200 | tr -d '\n' | cut -c1-120)"
"$MG" clone "file://$W/2/origin" "$W/2/mg-copy" >"$W/2/mg.out" 2>&1; echo "mg clone exit=$? out=$(cat "$W/2/mg.out")"
git clone -q "file://$W/2/origin" "$W/2/git-copy" 2>"$W/2/git.err"; echo "git clone exit=$? err=$(cat "$W/2/git.err")"
diff <(git -C "$W/2/mg-copy" show-ref | sort) <(git -C "$W/2/git-copy" show-ref | sort) && echo "show-ref IDENTICAL"
echo "mg  HEAD=$(git -C "$W/2/mg-copy" symbolic-ref HEAD)  origin/HEAD=$(git -C "$W/2/mg-copy" symbolic-ref refs/remotes/origin/HEAD)"
echo "git HEAD=$(git -C "$W/2/git-copy" symbolic-ref HEAD)  origin/HEAD=$(git -C "$W/2/git-copy" symbolic-ref refs/remotes/origin/HEAD)"
echo "mg  branch -a: $(git -C "$W/2/mg-copy" branch -a | tr '\n' '|')"
echo "git branch -a: $(git -C "$W/2/git-copy" branch -a | tr '\n' '|')"
echo "mg status: [$(git -C "$W/2/mg-copy" status --porcelain)] fsck:"; git -C "$W/2/mg-copy" fsck --no-progress; echo "fsck exit=$?"

echo
echo "############ (3) 远端 HEAD -> unborn 分支（只广告 refs/heads/main）：mg clone vs git clone"
mkdir -p "$W/3/origin"
git -C "$W/3/origin" init -q -b main .
echo a >"$W/3/origin/a.txt"; git -C "$W/3/origin" add -A; git -C "$W/3/origin" commit -qm one
git -C "$W/3/origin" symbolic-ref HEAD refs/heads/master
"$MG" clone "file://$W/3/origin" "$W/3/mg-copy" >"$W/3/mg.out" 2>"$W/3/mg.err"; echo "mg clone exit=$? err=[$(cat "$W/3/mg.err")]"
git clone "file://$W/3/origin" "$W/3/git-copy" >"$W/3/git.out" 2>"$W/3/git.err"; echo "git clone exit=$? err=[$(cat "$W/3/git.err")]"
diff <(git -C "$W/3/mg-copy" show-ref | sort) <(git -C "$W/3/git-copy" show-ref | sort) && echo "show-ref IDENTICAL"

echo
echo "############ (4) push -u：config 是否被真实 git 读得到"
mkdir -p "$W/4/origin"
git -C "$W/4/origin" init -q --bare -b main .
git -C "$W/4/origin" config receive.denyCurrentBranch refuse
mkdir -p "$W/4/seed"
git -C "$W/4/seed" init -q -b main .
echo a >"$W/4/seed/a.txt"; git -C "$W/4/seed" add -A; git -C "$W/4/seed" commit -qm one
git -C "$W/4/seed" push -q "file://$W/4/origin" refs/heads/main:refs/heads/main
"$MG" clone "file://$W/4/origin" "$W/4/work" >/dev/null 2>&1; echo "mg clone exit=$?"
echo "after clone: branch.main.remote=$(git -C "$W/4/work" config --get branch.main.remote) merge=$(git -C "$W/4/work" config --get branch.main.merge)"
git -C "$W/4/work" config --unset branch.main.remote 2>/dev/null; git -C "$W/4/work" config --unset branch.main.merge 2>/dev/null
echo b >"$W/4/work/b.txt"; git -C "$W/4/work" add -A; git -C "$W/4/work" commit -qm two
"$MG" push -u origin main >"$W/4/push.out" 2>&1; echo "mg push -u exit=$? out=[$(cat "$W/4/push.out")]"
echo "after push -u: remote=$(git -C "$W/4/work" config --get branch.main.remote) merge=$(git -C "$W/4/work" config --get branch.main.merge)"
echo "real git reads it: $(git -C "$W/4/work" rev-parse --abbrev-ref 'main@{upstream}')"
echo "bare main == work HEAD? $(git -C "$W/4/origin" rev-parse refs/heads/main) vs $(git -C "$W/4/work" rev-parse HEAD)"

echo
echo "############ (5) 非 bare 远端：当前检出分支必须被拒（denyCurrentBranch=refuse）"
mkdir -p "$W/5/nonbare"
git -C "$W/5/nonbare" init -q -b main .
echo a >"$W/5/nonbare/a.txt"; git -C "$W/5/nonbare" add -A; git -C "$W/5/nonbare" commit -qm one
mkdir -p "$W/5/clone-work"
git clone -q "file://$W/5/nonbare" "$W/5/clone-work"
echo b >"$W/5/clone-work/b.txt"; git -C "$W/5/clone-work" add -A; git -C "$W/5/clone-work" commit -qm two
echo "--- mg push to a non-bare remote whose main is checked out:"
"$MG" -C "$W/5/clone-work" push origin main; echo "mg exit=$?"
echo "--- real git push on the same fixture:"
git -C "$W/5/clone-work" push origin main 2>&1 | tail -3; echo "git exit=${PIPESTATUS[0]}"
echo "non-bare main unchanged? $(git -C "$W/5/nonbare" rev-parse refs/heads/main) vs clone HEAD $(git -C "$W/5/clone-work" rev-parse HEAD)"

echo
echo "############ (6) pull 的非快进路径（远端与本地分叉）"
mkdir -p "$W/6/origin"
git -C "$W/6/origin" init -q --bare -b main .
mkdir -p "$W/6/seed"; git -C "$W/6/seed" init -q -b main .
echo a >"$W/6/seed/a.txt"; git -C "$W/6/seed" add -A; git -C "$W/6/seed" commit -qm one
git -C "$W/6/seed" push -q "file://$W/6/origin" refs/heads/main:refs/heads/main
"$MG" clone "file://$W/6/origin" "$W/6/mg-work" >/dev/null 2>&1
git clone -q "file://$W/6/origin" "$W/6/git-work"
# 本地分歧提交
echo local >"$W/6/mg-work/local.txt"; git -C "$W/6/mg-work" add -A; git -C "$W/6/mg-work" commit -qm local
echo local >"$W/6/git-work/local.txt"; git -C "$W/6/git-work" add -A; git -C "$W/6/git-work" commit -qm local
# 远端前进（另一条线）
echo remote >"$W/6/seed/remote.txt"; git -C "$W/6/seed" add -A; git -C "$W/6/seed" commit -qm remote
git -C "$W/6/seed" push -q "file://$W/6/origin" refs/heads/main:refs/heads/main
echo "--- mg pull (非快进):"
"$MG" -C "$W/6/mg-work" pull origin; echo "mg pull exit=$?"
echo "--- git pull (同一夹具):"
git -C "$W/6/git-work" pull --no-rebase origin main 2>&1 | tail -3; echo "git pull exit=${PIPESTATUS[0]}"
echo "mg  HEAD=$(git -C "$W/6/mg-work" rev-parse HEAD) status=[$(git -C "$W/6/mg-work" status --porcelain | tr '\n' '|')]"
echo "git HEAD=$(git -C "$W/6/git-work" rev-parse HEAD) status=[$(git -C "$W/6/git-work" status --porcelain | tr '\n' '|')]"

echo
echo "############ (7) clone 默认目录名（URL 最后一段，去掉 .git）"
mkdir -p "$W/7" && cd "$W/7"
git -C "$W/7" init -q --bare -b main repo.git
"$MG" clone "file://$W/7/repo.git"; echo "mg exit=$?  ls: $(ls -a "$W/7")"
rm -rf "$W/7/repo"
git clone -q "file://$W/7/repo.git"; echo "git exit=$?  ls: $(ls -a "$W/7")"

echo
echo "############ (8) mg fetch <explicit refspec> / FETCH_HEAD 形态"
mkdir -p "$W/8/origin"; git -C "$W/8/origin" init -q -b main .
echo a >"$W/8/origin/a.txt"; git -C "$W/8/origin" add -A; git -C "$W/8/origin" commit -qm one
git -C "$W/8/origin" branch feature
git clone -q "file://$W/8/origin" "$W/8/git-work"
"$MG" clone "file://$W/8/origin" "$W/8/mg-work" >/dev/null
echo b >"$W/8/origin/b.txt"; git -C "$W/8/origin" add -A; git -C "$W/8/origin" commit -qm two
git -C "$W/8/git-work" fetch origin "+refs/heads/*:refs/remotes/origin/*"
"$MG" -C "$W/8/mg-work" fetch origin "+refs/heads/*:refs/remotes/origin/*"
echo "--- mg FETCH_HEAD:"; cat "$W/8/mg-work/.git/FETCH_HEAD"
echo "--- git FETCH_HEAD:"; cat "$W/8/git-work/.git/FETCH_HEAD"
echo "--- diff of FETCH_HEAD (url identical):"
diff "$W/8/mg-work/.git/FETCH_HEAD" "$W/8/git-work/.git/FETCH_HEAD" && echo "FETCH_HEAD IDENTICAL"
echo "--- mg fetch 后再 fetch 一次 objects 目标："
"$MG" -C "$W/8/mg-work" fetch origin "+refs/heads/*:refs/remotes/origin/*"; echo "second fetch exit=$?"
echo "--- refs/remotes/origin/main mg vs origin: $(git -C "$W/8/mg-work" rev-parse refs/remotes/origin/main) vs $(git -C "$W/8/origin" rev-parse refs/heads/main)"
