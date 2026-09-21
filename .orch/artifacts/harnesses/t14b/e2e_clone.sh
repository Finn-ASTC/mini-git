#!/usr/bin/env bash
set -uo pipefail
ROOT=/tmp/t14b/root3
rm -rf "$ROOT" /tmp/t14b/cap3.out /tmp/t14b/cap3.err /tmp/t14b/mgclone
mkdir -p "$ROOT"
export GIT_CONFIG_NOSYSTEM=1
mkdir -p "$ROOT/seed"
git -C "$ROOT/seed" init -q -b main .
printf 'hello from mg http\n' > "$ROOT/seed/a.txt"
git -C "$ROOT/seed" add a.txt
git -C "$ROOT/seed" -c user.name=t14 -c user.email=t14@example.com commit -qm c1
git -C "$ROOT" clone -q --bare seed srv.git
python3 /tmp/t14b/capture.py "$ROOT" >/tmp/t14b/cap3.out 2>/tmp/t14b/cap3.err &
SRV=$!
for i in $(seq 1 50); do
  PORT=$(grep -m1 '^PORT=' /tmp/t14b/cap3.err 2>/dev/null | cut -d= -f2)
  [ -n "${PORT:-}" ] && break
  sleep 0.1
done
URL="http://127.0.0.1:$PORT/srv.git"
echo "server=$URL"
/home/user/Projects/mini-git/target/debug/mg clone "$URL" /tmp/t14b/mgclone; echo "mg clone exit=$?"
echo "--- real git reads the mg clone ---"
git -C /tmp/t14b/mgclone fsck --no-progress; echo "fsck exit=$?"
git -C /tmp/t14b/mgclone log --oneline; git -C /tmp/t14b/mgclone status --porcelain
cat /tmp/t14b/mgclone/a.txt 2>/dev/null
sleep 0.3; kill "$SRV" 2>/dev/null; wait "$SRV" 2>/dev/null
echo "=== requests our mg client sent ==="
python3 - <<'PY'
import json
recs=None
for l in open('/tmp/t14b/cap3.out'):
    l=l.strip()
    if not l: continue
    try: obj=json.loads(l)
    except Exception: continue
    if 'records' in obj: recs=obj['records']
if recs is None: print('NO RECORDS')
else:
    for r in recs:
        print(f"----- seq={r['seq']} body_bytes={r['body_bytes']} -----")
        print(r['head'])
PY
