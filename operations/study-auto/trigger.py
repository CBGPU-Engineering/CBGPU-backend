#!/usr/bin/env python3
"""Wake the authenticated scheduler only during weekday application hours (KST)."""
import datetime,json,urllib.request,sys
from pathlib import Path
from zoneinfo import ZoneInfo
now=datetime.datetime.now(ZoneInfo('Asia/Seoul'))
minute=now.hour*60+now.minute
if now.weekday()>=5 or not 450<=minute<=930:
 print('Outside weekday automation window.');sys.exit(0)
secret=(Path.home()/'.config/cbgpu/study-auto-secret').read_text().strip()
request=urllib.request.Request('http://127.0.0.1:3000/api/cron/study-auto',data=b'{}',headers={'Host':'cbgpu.com','Authorization':'Bearer '+secret,'Content-Type':'application/json','User-Agent':'CBGPU-StudyScheduler/1.0'})
try:
 with urllib.request.urlopen(request,timeout=150) as response:result=json.load(response)
 print(json.dumps({'ok':result.get('ok',False),'processed':result.get('processed',0),'skipped':result.get('skipped')},ensure_ascii=False))
except Exception:
 print('Automatic study check failed; inspect service health. No credentials logged.',file=sys.stderr);sys.exit(1)
