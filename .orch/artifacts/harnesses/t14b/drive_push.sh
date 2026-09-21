#!/usr/bin/env bash
# Capture a real `git push` (receive-pack) request head.
set -uo pipefail
ROOT=/tmp/t14b/root2
rm -rf "$ROOT" /tmp/t14b/cap2.out /tmp/t14b/cap2.err
mkdir -p "$ROOT"
export GIT_CONFIG_NOSYSTEM=1
mkdir -p "$ROOT/seed"
git -C "$ROOT/seed" init -q -b main .
printf 'one\n' > "$ROOT/seed/f.txt"
git -C "$ROOT/seed" add f.txt
git -C "$ROOT/seed" -c user.name=t14 -c user.email=t14@example.com commit -qm c1
git -C "$ROOT" clone -q --bare seed srv.git
git -C "$ROOT/srv.git" config http.receivepack true
mkdir -p "$ROOT/cli"
git -C "$ROOT" clone -q srv.git cli
printf 'two\n' > "$ROOT/cli/f.txt"
git -C "$ROOT/cli" add f.txt
git -C "$ROOT/cli" -c user.name=t14 -c user.email=t14@example.com commit -qm c2

python3 /tmp/t14b/capture.py "$ROOT" >/tmp/t14b/cap2.out 2>/tmp/t14b/cap2.err &
SRV=$!
for i in $(seq 1 50); do
  PORT=$(grep -m1 '^PORT=' /tmp/t14b/cap2.err 2>/dev/null | cut -d= -f2)
  [ -n "${PORT:-}" ] && break
  sleep 0.1
done
URL="http://127.0.0.1:$PORT/srv.git"
echo "capture server port=$PORT"
echo "=== real git push ==="
git -C "$ROOT/cli" -c protocol.version=0 push -q "$URL" main; echo "push exit=$?"
sleep 0.5
kill "$SRV" 2>/dev/null; wait "$SRV" 2>/dev/null
echo "=== captured records ==="
python3 - <<'PY'
import json
recs=None
for l in open('/tmp/t14b/cap2.out'):
    l=l.strip()
    if not l: continue
    try: obj=json.loads(l)
    except Exception: continue
    if 'records' in obj: recs=obj['records']
if recs is None:
    print('NO RECORDS')
else:
    for r in recs:
        print(f"----- seq={r['seq']} body_bytes={r['body_bytes']} -----")
        print(r['head'])
PY
