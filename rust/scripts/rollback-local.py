#!/usr/bin/env python3
"""Restore previous worker units without stopping timers or touching user data."""
import json,shutil,subprocess,sys
from pathlib import Path
root=(Path.home()/'.local/share/cbgpu-rust/rollbacks').resolve()
if len(sys.argv)!=2:raise SystemExit('Usage: rollback-local.py ROLLBACK_DIRECTORY')
backup=Path(sys.argv[1]).resolve()
if backup.parent!=root:raise SystemExit('Refusing a rollback outside the private manifest directory.')
manifest=json.loads((backup/'manifest.json').read_text());units=Path.home()/'.config/systemd/user';base=Path.home()/'.local/lib/cbgpu-rust'
allowed={'cbgpu-rust-core.service','cbgpu-study-auto.service','cbgpu-timetable-refresh.service'}
if set(manifest['units'])!=allowed:raise SystemExit('Invalid rollback unit set.')
for name,existed in manifest['units'].items():
 if existed:shutil.copyfile(backup/name,units/name)
 else:(units/name).unlink(missing_ok=True)
previous=manifest['previous_release'];current=base/'current'
if previous:
 target=Path(previous).resolve()
 if target.parent!=(base/'releases').resolve():raise SystemExit('Invalid prior release path.')
 temp=base/'rollback';temp.unlink(missing_ok=True);temp.symlink_to(target);temp.replace(current)
else:current.unlink(missing_ok=True)
subprocess.run(['systemctl','--user','daemon-reload'],check=True)
subprocess.run(['systemctl','--user','restart' if previous else 'stop','cbgpu-rust-core.service'],check=True)
print(json.dumps({'ok':True,'timers_preserved':True,'user_services_restarted':False,'database_rolled_back':False}))
