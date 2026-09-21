#!/usr/bin/env bash
# Drive the capture harness with a real git client (ls-remote + fetch).
set -uo pipefail
ROOT=/tmp/t14b/root
rm -rf "$ROOT" /tmp/t14b/cap.out /tmp/t14b/cap.err
mkdir -p "$ROOT"
export GIT_CONFIG_NOSYSTEM=1
mkdir -p "$ROOT/seed"
git -C "$ROOT/seed" init -q -b main .
printf 'one\n' > "$ROOT/seed/f.txt"
git -C "$ROOT/seed" add f.txt
git -C "$ROOT/seed" -c user.name=t14 -c user.email=t14@example.com commit -qm c1
git -C "$ROOT" clone -q --bare seed srv.git
git -C "$ROOT/srv.git" config http.receivepack true

python3 /tmp/t14b/capture.py "$ROOT" >/tmp/t14b/cap.out 2>/tmp/t14b/cap.err &
SRV=$!
for i in $(seq 1 50); do
  PORT=$(grep -m1 '^PORT=' /tmp/t14b/cap.err 2>/dev/null | cut -d= -f2)
  [ -n "${PORT:-}" ] && break
  sleep 0.1
done
echo "capture server port=$PORT pid=$SRV"
URL="http://127.0.0.1:$PORT/srv.git"

echo "=== real git ls-remote ==="
git -c protocol.version=0 ls-remote "$URL"; echo "ls-remote exit=$?"

mkdir -p "$ROOT/client"
git -C "$ROOT/client" init -q -b main
echo "=== real git fetch ==="
git -C "$ROOT/client" -c protocol.version=0 fetch -q "$URL" HEAD; echo "fetch exit=$?"

sleep 0.5
kill "$SRV" 2>/dev/null
wait "$SRV" 2>/dev/null
echo "=== captured records ==="
python3 - <<'PY'
import json
lines=[l for l in open('/tmp/t14b/cap.out') if l.strip()]
recs=None
for l in lines:
    try:
        obj=json.loads(l)
    except Exception:
        continue
    if 'records' in obj:
        recs=obj['records']
if recs is None:
    print('NO RECORDS')
else:
    for r in recs:
        print(f"----- record seq={r['seq']} body_bytes={r['body_bytes']} -----")
        print(r['head'])
PY
