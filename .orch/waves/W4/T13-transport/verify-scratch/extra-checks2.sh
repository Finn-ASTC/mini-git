#!/usr/bin/env bash
# V13 追加检查（第二轮）：修正上一轮的脚本 bug（push -u 的 cwd），并深挖两个待判定项：
#   (A) 空远端：mg 报错，真实 git 却 exit 0；以及 pull 非快进的结果对比（tree 级）。
set -uo pipefail

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@e GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@e
export GIT_AUTHOR_DATE='1700000000 +0000' GIT_COMMITTER_DATE='1700000000 +0000'
export LC_ALL=C

MG=/home/user/Projects/mini-git/target/debug/mg
W=/tmp/v13extra2
rm -rf "$W"; mkdir -p "$W"

echo "############ (4b) push -u：cwd 修正后重跑"
mkdir -p "$W/4/origin" "$W/4/seed"
git -C "$W/4/origin" init -q --bare -b main .
git -C "$W/4/seed" init -q -b main .
echo a >"$W/4/seed/a.txt"; git -C "$W/4/seed" add -A; git -C "$W/4/seed" commit -qm one
git -C "$W/4/seed" push -q "file://$W/4/origin" refs/heads/main:refs/heads/main
"$MG" clone "file://$W/4/origin" "$W/4/work" >/dev/null 2>&1; echo "clone exit=$?"
echo "clone 后的 config: remote=$(git -C "$W/4/work" config --get branch.main.remote) merge=$(git -C "$W/4/work" config --get branch.main.merge)"
git -C "$W/4/work" config --unset branch.main.remote; git -C "$W/4/work" config --unset branch.main.merge
echo "unset 后: remote=[$(git -C "$W/4/work" config --get branch.main.remote)] merge=[$(git -C "$W/4/work" config --get branch.main.merge)]"
echo b >"$W/4/work/b.txt"; git -C "$W/4/work" add -A; git -C "$W/4/work" commit -qm two
"$MG" -C "$W/4/work" push -u origin main; echo "mg push -u exit=$?"
echo "push -u 后: remote=$(git -C "$W/4/work" config --get branch.main.remote) merge=$(git -C "$W/4/work" config --get branch.main.merge)"
echo "真实 git 读 upstream: $(git -C "$W/4/work" rev-parse --abbrev-ref 'main@{upstream}')"
echo "bare main == work HEAD? $(git -C "$W/4/origin" rev-parse refs/heads/main) vs $(git -C "$W/4/work" rev-parse HEAD)"
echo "--- mg push -u 之后真实 git 无参数 push（依赖 upstream）："
git -C "$W/4/work" push 2>&1 | tail -2; echo "git push exit=${PIPESTATUS[0]}"

echo
echo "############ (6b) pull 非快进：mg 与真实 git 的 merge 结果（tree 级对比）"
mkdir -p "$W/6/origin" "$W/6/seed"
git -C "$W/6/origin" init -q --bare -b main .
git -C "$W/6/seed" init -q -b main .
echo a >"$W/6/seed/a.txt"; git -C "$W/6/seed" add -A; git -C "$W/6/seed" commit -qm one
git -C "$W/6/seed" push -q "file://$W/6/origin" refs/heads/main:refs/heads/main
"$MG" clone "file://$W/6/origin" "$W/6/mg-work" >/dev/null 2>&1
git clone -q "file://$W/6/origin" "$W/6/git-work"
echo local >"$W/6/mg-work/local.txt"; git -C "$W/6/mg-work" add -A; git -C "$W/6/mg-work" commit -qm local
echo local >"$W/6/git-work/local.txt"; git -C "$W/6/git-work" add -A; git -C "$W/6/git-work" commit -qm local
echo remote >"$W/6/seed/remote.txt"; git -C "$W/6/seed" add -A; git -C "$W/6/seed" commit -qm remote
git -C "$W/6/seed" push -q "file://$W/6/origin" refs/heads/main:refs/heads/main
echo "--- mg pull -v 输出与退出码："
"$MG" -C "$W/6/mg-work" pull origin; echo "mg pull exit=$?"
echo "--- git pull 输出与退出码："
git -C "$W/6/git-work" pull --no-rebase origin main; echo "git pull exit=${PIPESTATUS[0]}"
echo "mg  log: $(git -C "$W/6/mg-work" log --oneline | tr '\n' '|')"
echo "git log: $(git -C "$W/6/git-work" log --oneline | tr '\n' '|')"
echo "mg  HEAD parents=$(git -C "$W/6/mg-work" rev-list --parents -n1 HEAD)"
echo "git HEAD parents=$(git -C "$W/6/git-work" rev-list --parents -n1 HEAD)"
echo "mg  tree=$(git -C "$W/6/mg-work" rev-parse HEAD^{tree})"
echo "git tree=$(git -C "$W/6/git-work" rev-parse HEAD^{tree})"
echo "mg  ls-tree: $(git -C "$W/6/mg-work" ls-tree -r --name-only HEAD | tr '\n' '|')"
echo "git ls-tree: $(git -C "$W/6/git-work" ls-tree -r --name-only HEAD | tr '\n' '|')"
echo "mg  status=[$(git -C "$W/6/mg-work" status --porcelain | tr '\n' '|')]"
echo "mg  fsck:"; git -C "$W/6/mg-work" fsck --no-progress; echo "fsck exit=$?"
echo "mg  remote-tracking: $(git -C "$W/6/mg-work" rev-parse refs/remotes/origin/main) vs origin $(git -C "$W/6/origin" rev-parse refs/heads/main)"

echo
echo "############ (7b) 空远端：真实 git 的行为 vs mg 的行为（夹具完整状态）"
# bare 空远端
mkdir -p "$W/7"
git -C "$W/7" init -q --bare -b main empty-bare.git
echo "--- empty bare remote: files = $(ls -A "$W/7/empty-bare.git" | tr '\n' ' ')"
git clone "file://$W/7/empty-bare.git" "$W/7/git-clone-of-empty-bare" 2>&1; echo "git clone exit=$?"
echo "git 产物: $(ls -A "$W/7/git-clone-of-empty-bare" 2>/dev/null | tr '\n' ' ')"
"$MG" clone "file://$W/7/empty-bare.git" "$W/7/mg-clone-of-empty-bare" 2>&1; echo "mg clone exit=$?"
echo "mg 产物存在? $(test -e "$W/7/mg-clone-of-empty-bare" && echo yes || echo no)  dir 内容: $(ls -A "$W/7/mg-clone-of-empty-bare" 2>/dev/null | tr '\n' ' ')"
echo "--- 对照：mg init 后 git 认为它是空仓库？"
"$MG" init "$W/7/mg-empty" >/dev/null 2>&1; git -C "$W/7/mg-empty" symbolic-ref HEAD; git -C "$W/7/mg-empty" status --porcelain | head -2
echo "--- 真实 git 把「有 HEAD 但无引用」的非 bare 仓库叫什么：git clone 的警告文本见上"
# 非 bare 空远端
mkdir -p "$W/7/empty-work"
git -C "$W/7/empty-work" init -q -b main .
git clone "file://$W/7/empty-work" "$W/7/git-clone-of-empty-work" 2>&1; echo "git clone (non-bare empty) exit=$?"
"$MG" clone "file://$W/7/empty-work" "$W/7/mg-clone-of-empty-work" 2>&1; echo "mg clone (non-bare empty) exit=$?"
echo "mg 产物存在? $(test -e "$W/7/mg-clone-of-empty-work" && echo yes || echo no)"

echo
echo "############ (9) 空远端上的 fetch / pull（mg）"
mkdir -p "$W/9"; "$MG" init "$W/9/repo" >/dev/null 2>&1
echo "mg fetch 空远端: $(cd "$W/9/repo" && "$MG" fetch "file://$W/7/empty-bare.git" 2>&1; echo exit=$?)"
echo "mg pull 空远端:  $(cd "$W/9/repo" && "$MG" pull "file://$W/7/empty-bare.git" 2>&1; echo exit=$?)"
echo "对照 git fetch 空远端: $(cd "$W/9/repo" && git fetch "file://$W/7/empty-bare.git" 2>&1; echo exit=$?)"
