#!/usr/bin/env bash
# Controller-side manual supervision (watch.py blocked by SKILL-FINDINGS P1).
#   orch-supervise.sh status | tail <pane> [n] | watch [secs] [interval] | adopt <targets-file>
set -uo pipefail
S=default
TF="${ORCH_TARGETS:-/tmp/orch-targets.txt}"
cd /home/user/Projects/mini-git || exit 2
mapfile -t targets < <(grep -v '^\s*#' "$TF" | grep -v '^\s*$')
status() {
  printf '== %s ==\n' "$(date +%H:%M:%S)"
  for t in "${targets[@]}"; do
    agent="${t%%|*}"; rest="${t#*|}"; pane="${rest%%|*}"; result="${rest#*|}"
    st=$(herdr --session $S agent get "$agent" 2>/dev/null | python3 -c 'import json,sys
try:
    a=json.load(sys.stdin)["result"]["agent"]; print("state=%s rev=%s" % (a.get("agent_status"), a.get("revision")))
except Exception: print("agent-get-failed")')
    if [ -f "$result" ]; then
      rs="RESULT: $(python3 -c 'import json,sys;d=json.load(open(sys.argv[1]));print(d.get("status"),d.get("completed_at"))' "$result" 2>/dev/null || echo unparsable)"
    else rs="no-result"; fi
    printf '[%-16s] %-7s %s  %s\n' "$agent" "$pane" "$st" "$rs"
  done
}
case "${1:-status}" in
  status) status ;;
  tail) shift; herdr --session $S pane read "$1" --source visible --lines "${2:-40}" 2>&1 | tail -n "${2:-40}" ;;
  watch)
    dur="${2:-120}"; iv="${3:-30}"; end=$(( $(date +%s) + dur ))
    declare -A seen=()
    for t in "${targets[@]}"; do result="${t##*|}"; [ -f "$result" ] && seen["$result"]=1; done
    while :; do
      status
      for t in "${targets[@]}"; do
        result="${t##*|}"
        [ -z "${seen[$result]:-}" ] && [ -f "$result" ] && { echo "NEW RESULT -> $result"; exit 0; }
      done
      [ "$(date +%s)" -ge "$end" ] && { echo "watch window ended"; exit 0; }
      sleep "$iv"
    done ;;
esac
