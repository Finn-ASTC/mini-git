#!/usr/bin/env bash
# 用法: orch-reuse.sh <wave> <slug> <label> <pane> <ws> <first-msg>
# 在【既存 agent 会话】里再开一个 round（不新建 space）：prepare + 记录 resources + 投递 prompt
set -euo pipefail
WAVE="$1"; SLUG="$2"; LABEL="$3"; PANE="$4"; WS="$5"; KINDHINT="${6:-}"
PROJ="/home/user/Projects/mini-git"
ROUNDS="$PROJ/.orch/rounds/$WAVE"
TASKFILE="$PROJ/.orch/waves/$WAVE/$SLUG/verify-task.md"
mkdir -p "$ROUNDS" "$PROJ/.orch/waves/$WAVE/$SLUG/verify-scratch"
R=$(python3 "$HOME/.agents/skills/agent-orchestrator/scripts/protocol.py" prepare \
      --cwd "$PROJ" --parent-depth 0 --max-depth 3 --task-file "$TASKFILE" \
      --brief --root "$ROUNDS")
REQ=$(echo "$R" | python3 -c 'import json,sys;print(json.load(sys.stdin)["request_path"])')
PROMPT=$(echo "$R" | python3 -c 'import json,sys;print(json.load(sys.stdin)["prompt_path"])')
RUNDIR=$(dirname "$REQ")
TAB=$(herdr --session default workspace list | python3 -c "
import json,sys
d=json.load(sys.stdin)['result']['workspaces']
print(next((w['active_tab_id'] for w in d if w['workspace_id']=='$WS'), ''))")
/tmp/orch-record.py "$RUNDIR" "$LABEL" "$PANE" "$WS" "$TAB" >/dev/null
printf '%s|%s|%s\n' "$LABEL" "$PANE" "$RUNDIR/result.json" >> /tmp/orch-targets.txt
MSG="$PROMPT"
[ -n "$KINDHINT" ] && MSG="$KINDHINT"$'\n\n'"$PROMPT"
herdr --session default agent prompt "$LABEL" "$MSG" >/dev/null
echo "reused $LABEL pane=$PANE round=$RUNDIR"
