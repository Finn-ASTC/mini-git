#!/usr/bin/env bash
# 用法: orch-launch.sh <wave> <slug> <label> <kind> <role:author|verify>
# 例:   orch-launch.sh W3 T9-pack w3-t9-codex codex author
set -euo pipefail
WAVE="$1"; SLUG="$2"; LABEL="$3"; KIND="$4"; ROLE="${5:-author}"
PROJ="/home/user/Projects/mini-git"
ROUNDS="$PROJ/.orch/rounds/$WAVE"
TASKFILE="$PROJ/.orch/waves/$WAVE/$SLUG/$([ "$ROLE" = author ] && echo task.md || echo verify-task.md)"
mkdir -p "$ROUNDS" "$PROJ/.orch/waves/$WAVE/$SLUG/verify-scratch"
R=$(python3 "$HOME/.agents/skills/agent-orchestrator/scripts/protocol.py" prepare \
      --cwd "$PROJ" --parent-depth 0 --max-depth 3 --task-file "$TASKFILE" \
      --brief --root "$ROUNDS")
REQ=$(echo "$R" | python3 -c 'import json,sys;print(json.load(sys.stdin)["request_path"])')
PROMPT=$(echo "$R" | python3 -c 'import json,sys;print(json.load(sys.stdin)["prompt_path"])')
RUNDIR=$(dirname "$REQ")
read -r WS PANE TAB < <(herdr --session default workspace create --cwd "$PROJ" \
    --label "$LABEL" --no-focus | python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print(r["workspace"]["workspace_id"], r["root_pane"]["pane_id"], r["tab"]["tab_id"])')
herdr --session default agent start "$LABEL" --kind "$KIND" --pane "$PANE" >/dev/null
sleep 4
/tmp/orch-record.py "$RUNDIR" "$LABEL" "$PANE" "$WS" "$TAB" >/dev/null
printf '%s|%s|%s\n' "$LABEL" "$PANE" "$RUNDIR/result.json" >> /tmp/orch-targets.txt
herdr --session default agent prompt "$LABEL" "$(cat "$PROMPT")" >/dev/null
echo "launched $LABEL kind=$KIND role=$ROLE ws=$WS pane=$PANE round=$RUNDIR"
