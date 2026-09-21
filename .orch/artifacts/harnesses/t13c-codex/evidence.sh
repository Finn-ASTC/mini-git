#!/usr/bin/env bash
# T13c 原始对拍证据（mg vs 真实 git）。不 export 任何 GIT_*；夹具全在 /tmp/t13c-codex/。
set -u
MG=/home/user/Projects/mini-git/target/debug/mg
ROOT=/tmp/t13c-codex/evidence
rm -rf "$ROOT"; mkdir -p "$ROOT"

for kind in bare plain; do
  src="$ROOT/remote-$kind"
  mkdir -p "$src"
  if [ "$kind" = bare ]; then
    git -C "$src" init -q --bare -b main .
  else
    git -C "$src" init -q -b main .
  fi
  url="file://$src"
  mine="$ROOT/mine-$kind"; real="$ROOT/real-$kind"
  mkdir -p "$mine" "$real"
  echo "############ fixture: $kind   url=$url"
  echo "--- remote: git show-ref"; git -C "$src" show-ref; echo "exit=$?"
  echo "--- remote: git symbolic-ref HEAD"; git -C "$src" symbolic-ref HEAD
  echo "--- mg clone:"
  ( cd "$mine" && "$MG" clone "$url" clone >stdout.txt 2>stderr.txt; echo "mg clone exit=$?" )
  echo "--- git clone:"
  ( cd "$real" && git clone "$url" clone >stdout.txt 2>stderr.txt; echo "git clone exit=$?" )
  echo "--- mg stdout:"; cat "$mine/stdout.txt"
  echo "--- mg stderr:"; cat "$mine/stderr.txt"
  echo "--- git stdout:"; cat "$real/stdout.txt"
  echo "--- git stderr:"; cat "$real/stderr.txt"
  if diff "$mine/stderr.txt" "$real/stderr.txt" >/dev/null; then echo "--- stderr: IDENTICAL (byte-for-byte)"; else echo "--- stderr: DIFFERS"; diff "$mine/stderr.txt" "$real/stderr.txt"; fi
  if diff "$mine/stdout.txt" "$real/stdout.txt" >/dev/null; then echo "--- stdout: IDENTICAL (byte-for-byte)"; else echo "--- stdout: DIFFERS"; diff "$mine/stdout.txt" "$real/stdout.txt"; fi
  echo "--- mg target \`ls -A\`:"; ls -A "$mine/clone"
  echo "--- git target \`ls -A\`:"; ls -A "$real/clone"
  echo "--- mg: git show-ref"; git -C "$mine/clone" show-ref; echo "exit=$?"
  echo "--- git: git show-ref"; git -C "$real/clone" show-ref; echo "exit=$?"
  echo "--- mg: git symbolic-ref HEAD"; git -C "$mine/clone" symbolic-ref HEAD
  echo "--- git: git symbolic-ref HEAD"; git -C "$real/clone" symbolic-ref HEAD
  echo "--- mg: git status --porcelain"; git -C "$mine/clone" status --porcelain; echo "exit=$?"
  echo "--- git: git status --porcelain"; git -C "$real/clone" status --porcelain; echo "exit=$?"
  echo "--- mg: .git/HEAD"; cat "$mine/clone/.git/HEAD"
  echo "--- git: .git/HEAD"; cat "$real/clone/.git/HEAD"
  echo "--- mg: git config --list --local:"; git -C "$mine/clone" config --list --local
  echo "--- git: git config --list --local:"; git -C "$real/clone" config --list --local
  echo "--- mg: git rev-parse --verify HEAD"; git -C "$mine/clone" rev-parse --verify HEAD; echo "exit=$?"
  echo "--- git: git rev-parse --verify HEAD"; git -C "$real/clone" rev-parse --verify HEAD; echo "exit=$?"
  echo "--- mg: git fsck --strict --no-progress"; git -C "$mine/clone" fsck --strict --no-progress; echo "exit=$?"
  echo "--- mg: FETCH_HEAD / refs/remotes/origin/HEAD"; ls "$mine/clone/.git/FETCH_HEAD" "$mine/clone/.git/refs/remotes/origin/HEAD" 2>&1
  echo "--- git: FETCH_HEAD / refs/remotes/origin/HEAD"; ls "$real/clone/.git/FETCH_HEAD" "$real/clone/.git/refs/remotes/origin/HEAD" 2>&1
  echo "--- mg: worktree files (non-.git)"; find "$mine/clone" -mindepth 1 -name .git -prune -o -print
  echo "--- fetch on the empty remote ---"
  ( cd "$mine/clone" && "$MG" fetch origin >fout.txt 2>ferr.txt; echo "mg fetch origin exit=$?" )
  ( cd "$real/clone" && git fetch origin >fout.txt 2>ferr.txt; echo "git fetch origin exit=$?" )
  echo "--- mg fetch stdout:"; cat "$mine/clone/fout.txt"; echo "--- mg fetch stderr:"; cat "$mine/clone/ferr.txt"
  echo "--- git fetch stdout:"; cat "$real/clone/fout.txt"; echo "--- git fetch stderr:"; cat "$real/clone/ferr.txt"
  echo "--- mg after fetch: show-ref"; git -C "$mine/clone" show-ref; echo "exit=$?"
  echo "--- git after fetch: show-ref"; git -C "$real/clone" show-ref; echo "exit=$?"
  echo "--- mg after fetch: FETCH_HEAD"; ls -l "$mine/clone/.git/FETCH_HEAD" 2>&1
  echo "--- git after fetch: FETCH_HEAD"; ls -l "$real/clone/.git/FETCH_HEAD" 2>&1
  echo "--- mg fetch stderr vs git fetch stderr:"; diff "$mine/clone/ferr.txt" "$real/clone/ferr.txt" && echo "(fetch stderr identical)"
  echo "--- mg fetch stdout vs git fetch stdout:"; diff "$mine/clone/fout.txt" "$real/clone/fout.txt" && echo "(fetch stdout identical)"
done
