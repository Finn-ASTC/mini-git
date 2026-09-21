#!/usr/bin/env python3
"""写一个 round 的 submission.json。用法: orch-submit.py <round_dir> <agent> <kind> <task> [evidence]"""
import json,os,sys,datetime
r,agent,kind,task = sys.argv[1:5]
ev = sys.argv[5] if len(sys.argv)>5 else "herdr agent prompt -> agent_prompted"
req=json.load(open(os.path.join(r,'request.json')))
now=datetime.datetime.now().astimezone().replace(microsecond=0).isoformat()
sub={"active_request":os.path.join(r,'request.json'),"job_id":req["job_id"],"round_id":req["round_id"],
     "transport":"insider (default session)","session":"default","agent":agent,"pane":os.environ.get("PANE","wJ:p1"),
     "workspace":os.environ.get("WS","wJ"),"cwd":req["cwd"],"kind":kind,"task":task,
     "submitted_at":now,"acceptance":"accepted","submission_evidence":ev,"deadline":None,
     "monitor_owner":"controller (default session, pane wJ:p1)","last_checked_at":now,"next_check_at":None,
     "pending_question":None,"resources_file":os.path.join(r,'resources.json'),"result_path":req["result_path"]}
json.dump(sub,open(os.path.join(r,'submission.json'),'w'),ensure_ascii=False,indent=2)
print("wrote",os.path.join(r,'submission.json'))
