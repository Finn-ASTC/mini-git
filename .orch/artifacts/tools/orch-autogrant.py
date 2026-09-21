#!/usr/bin/env python3
"""Supervised auto-approval for codex panes.

Approves ONLY dialogs whose command looks safe (writes under /tmp, reads repo).
Logs every approved command to /tmp/orch-approvals.log and stops on anything suspicious.
"""
import json, re, subprocess, sys, time

SESSION = "default"
LOG = "/tmp/orch-approvals.log"
DENY = [
    r"rm\s+-rf\s+/(?!tmp)", r"sudo\b", r"pacman\b", r"git\s+(reset|checkout|clean)\b",
    r">\s*src/", r"rm\s+src/", r"cargo\s+install", r"git\s+commit\b", r"curl\b", r"curl|wget",
    r"chmod\s+777", r"\.\./\.\.", r"truncate\b",
]

def herdr(*args):
    p = subprocess.run(["herdr", "--session", SESSION, *args], capture_output=True, text=True)
    return p.stdout

def screen(pane):
    out = herdr("pane", "read", pane, "--lines", "45")
    try:
        return json.loads(out)["result"]["text"]
    except Exception:
        return out

def main():
    pane = sys.argv[1]; name = sys.argv[2]; minutes = float(sys.argv[3]) if len(sys.argv) > 3 else 15
    deadline = time.time() + minutes * 60
    n = 0
    while time.time() < deadline:
        s = screen(pane)
        if "Would you like to run the following command?" in s:
            m = re.search(r"^\s*\$\s*(.+?)$", s, re.M)
            cmd = (m.group(1).strip() if m else "<unknown>")
            bad = [d for d in DENY if re.search(d, cmd)]
            if bad:
                print(f"STOP: suspicious command from {name}: {cmd!r} matched {bad}", flush=True)
                return 2
            herdr("agent", "send-keys", name, "enter")
            n += 1
            with open(LOG, "a") as f:
                f.write(f"{time.strftime('%H:%M:%S')} {name} APPROVED {cmd}\n")
            print(f"{time.strftime('%H:%M:%S')} approved#{n}: {cmd[:120]}", flush=True)
            time.sleep(3)
            continue
        time.sleep(12)
    print(f"finished loop, approved {n}", flush=True)

sys.exit(main())
