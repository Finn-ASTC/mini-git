#!/usr/bin/env bash
# V10 证据脚本：手工平行仓库对拍一例（UU + DU/UD + 干净路径），把两侧原始输出落到
# verify-scratch/evidence-conflict.txt，供 result 引用。
set -uo pipefail
OUT="$(cd "$(dirname "$0")" && pwd)/evidence-conflict.txt"
MG=/home/user/Projects/mini-git/target/debug/mg
WORK=$(mktemp -d /tmp/v10-evidence.XXXXXX)

mk_repo () {
  local dir="$1"
  mkdir -p "$dir"
  git -C "$dir" init -q -b main .
  git -C "$dir" config user.name "V10 verifier"
  git -C "$dir" config user.email "v10@example.invalid"
  printf '1\n2\n3\n' > "$dir/uu.txt"
  printf 'a\nb\nc\n' > "$dir/du.txt"
  printf 'a\nb\nc\n' > "$dir/ud.txt"
  printf 'clean\n'     > "$dir/clean.txt"
  git -C "$dir" add -A
  git -C "$dir" commit -qm base
  git -C "$dir" checkout -qb feature
  printf '1\nTHEIRS\n3\n' > "$dir/uu.txt"
  printf 'a\nTHEIRS\nc\n' > "$dir/du.txt"
  rm "$dir/ud.txt"
  printf 'clean\n' > "$dir/clean.txt"
  git -C "$dir" add -A
  git -C "$dir" commit -qm theirs
  git -C "$dir" checkout -q main
  printf '1\nOURS\n3\n' > "$dir/uu.txt"
  rm "$dir/du.txt"
  printf 'a\nOURS\nc\n' > "$dir/ud.txt"
  printf 'clean\n''ours line\n' > "$dir/clean.txt"
  git -C "$dir" add -A
  git -C "$dir" commit -qm ours
}

snapshot () {
  local dir="$1" label="$2" log="$3" codefile="$4"
  {
    echo "########## $label"
    echo "\$ mg/git merge feature  (exit code below)"
    echo "stdout+stderr:"
    cat "$log" | sed 's/^/    /'
    echo "exit code: $(cat "$codefile")"
    echo "\$ git status --porcelain"
    git -C "$dir" -c core.quotepath=false status --porcelain | sed 's/^/    /'
    echo "\$ git ls-files -u (unmerged stages)"
    git -C "$dir" ls-files -u | sed 's/^/    /'
    for f in uu.txt du.txt ud.txt; do
      if [ -f "$dir/$f" ]; then
        echo "---- worktree file: $f"
        cat -A "$dir/$f" | sed 's/^/    /'
        echo "    sha256: $(sha256sum "$dir/$f" | cut -d' ' -f1)"
      else
        echo "---- worktree file: $f  (absent)"
      fi
    done
    echo "---- clean.txt: $(cat "$dir/clean.txt" | tr '\n' '|')"
    echo "---- .git/MERGE_HEAD: $(cat "$dir/.git/MERGE_HEAD" 2>/dev/null | tr -d '\n')"
    echo "---- .git/MERGE_MSG : $(head -1 "$dir/.git/MERGE_MSG" 2>/dev/null)"
    echo "\$ git fsck --no-progress"
    git -C "$dir" fsck --no-progress 2>&1 | sed 's/^/    /'
  } >> "$OUT"
}

{
  echo "V10 证据：手工平行仓库对拍（UU + DU + UD + 干净路径）"
  echo "date: $(date -Is)"
  echo "git : $(git --version)"
  echo "mg  : $MG"
  echo
} > "$OUT"

mk_repo "$WORK/base"
cp -a "$WORK/base" "$WORK/mg-repo"
cp -a "$WORK/base" "$WORK/git-repo"

( cd "$WORK/mg-repo" && "$MG" merge feature > "$WORK/mg.out" 2>&1; echo $? > "$WORK/mg.code" )
snapshot "$WORK/mg-repo" "A: mg merge feature" "$WORK/mg.out" "$WORK/mg.code"
echo >> "$OUT"
( cd "$WORK/git-repo" && git merge --no-edit feature > "$WORK/git.out" 2>&1; echo $? > "$WORK/git.code" )
snapshot "$WORK/git-repo" "B: 真实 git merge feature（真值）" "$WORK/git.out" "$WORK/git.code"

{
  echo
  echo "########## 逐字节比较"
  for f in uu.txt du.txt ud.txt; do
    if cmp -s "$WORK/mg-repo/$f" "$WORK/git-repo/$f"; then
      echo "  $f: IDENTICAL"
    else
      echo "  $f: DIFFER"
    fi
  done
  echo "  stage diff (git ls-files -u):"
  diff <(git -C "$WORK/mg-repo" ls-files -u) <(git -C "$WORK/git-repo" ls-files -u) && echo "    IDENTICAL"
  echo "  porcelain diff:"
  diff <(git -C "$WORK/mg-repo" status --porcelain) <(git -C "$WORK/git-repo" status --porcelain) && echo "    IDENTICAL"
  echo
  echo "########## 真实 git 在 mg 留下的状态上收尾"
  ( cd "$WORK/mg-repo" && printf 'RESOLVED\n' > uu.txt && printf 'RESOLVED\n' > du.txt && printf 'RESOLVED\n' > ud.txt \
      && git add -A && git commit --no-edit -q && echo "    git commit OK" \
      && echo "    parents: $(git log --format=%P -1)" \
      && echo "    subject: $(git log --format=%s -1)" \
      && echo "    status : '$(git status --porcelain)'"
  )
  ( cd "$WORK/git-repo" && printf 'RESOLVED\n' > uu.txt && printf 'RESOLVED\n' > du.txt && printf 'RESOLVED\n' > ud.txt \
      && git add -A && git commit --no-edit -q \
      && echo "    oracle tree: $(git rev-parse HEAD^{tree})" )
  echo "    mg-repo tree: $(git -C "$WORK/mg-repo" rev-parse HEAD^{tree})"
} >> "$OUT"

echo "evidence written to $OUT"
echo "workdir kept at $WORK"
tail -30 "$OUT"
