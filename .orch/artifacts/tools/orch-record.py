#!/usr/bin/env python3
"""D1 变通：protocol.py record 拒字母型 pane ID（wN:p1），按同 schema 手工落盘 resources.json。
用法: orch-record.py <round_dir> <agent> <pane> <workspace> <tab> [parent_pane] [parent_tab]
"""
import json, sys, datetime, pathlib
rd, agent, pane, ws, tab = sys.argv[1:6]
parent_pane = sys.argv[6] if len(sys.argv) > 6 else "wJ:p1"
parent_tab = sys.argv[7] if len(sys.argv) > 7 else "wJ:t1"
rp = pathlib.Path(rd)
req = json.loads((rp / "request.json").read_text())
rec = {
    "mode": "insider", "session": "default", "agent": agent, "pane": pane, "workspace": ws,
    "owns_agent": True, "owns_pane": True, "owns_workspace": True, "owns_session": False,
    "tab": tab, "parent_pane": parent_pane, "parent_tab": parent_tab, "owns_tab": False,
    "recorded_at": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="microseconds"),
    "job_id": req["job_id"], "recorded_by": "controller-manual",
    "record_note": "protocol.py record 拒绝字母型 herdr ID（w[0-9]+:p[0-9]+），见 .orch/SKILL-FINDINGS.md P1；本文件按同 schema 手工落盘",
}
p = rp / "resources.json"
p.write_text(json.dumps(rec, ensure_ascii=False, indent=2) + "\n")
p.chmod(0o600)
print("recorded", p, "job", req["job_id"][:8])
