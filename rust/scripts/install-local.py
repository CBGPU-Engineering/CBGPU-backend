#!/usr/bin/env python3
"""Install verified Rust operations without restarting any user-facing service."""
import hashlib,json,os,shutil,subprocess,time,urllib.request
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];HOME=Path.home();BASE=HOME/'.local/lib/cbgpu-rust';UNITS=HOME/'.config/systemd/user'
NAMES=['cbgpu-rust-core.service','cbgpu-study-auto.service','cbgpu-timetable-refresh.service']
def system(*args):subprocess.run(['systemctl','--user',*args],check=True,capture_output=True)
def main():
 os.umask(0o077)
 binary=ROOT/'target/release/cbgpu-core'
 if not binary.is_file():raise SystemExit('Build the release binary first.')
 env=dict(line.split('=',1) for line in (HOME/'.config/cbgpu/rust-core.env').read_text().splitlines() if '=' in line)
 for command in ['study-auto','timetable-refresh']:
  subprocess.run([str(binary),command,'--dry-run'],env={**os.environ,**env},check=True,capture_output=True,timeout=15)
 digest=hashlib.sha256(binary.read_bytes()).hexdigest();release=BASE/'releases'/digest[:20];release.mkdir(parents=True,exist_ok=True)
 target=release/'cbgpu-core'
 if not target.exists():shutil.copy2(binary,target);target.chmod(0o755)
 assert hashlib.sha256(target.read_bytes()).hexdigest()==digest
 backup=HOME/'.local/share/cbgpu-rust/rollbacks'/time.strftime('%Y%m%dT%H%M%S');backup.mkdir(parents=True)
 current=BASE/'current';manifest={'previous_release':str(current.resolve()) if current.exists() else None,'units':{}}
 for name in NAMES:
  path=UNITS/name
  if path.exists():shutil.copy2(path,backup/name);manifest['units'][name]=True
  else:manifest['units'][name]=False
 (backup/'manifest.json').write_text(json.dumps(manifest));temp=BASE/'candidate';temp.unlink(missing_ok=True);temp.symlink_to(release);temp.replace(current)
 try:
  for name in NAMES:
   temp=UNITS/(name+'.rust-candidate');shutil.copyfile(ROOT/'deploy'/name,temp);temp.replace(UNITS/name)
  system('daemon-reload');system('enable','--now','cbgpu-rust-core.service')
  for _ in range(30):
   try:
    with urllib.request.urlopen('http://127.0.0.1:3540/health/ready',timeout=2)as r:health=json.load(r)
    if health.get('status')=='ok':break
   except Exception:time.sleep(0.5)
  else:raise RuntimeError('Rust readiness failed')
  # Existing timers remain enabled and keep their original intervals. An already
  # running oneshot finishes normally; the next activation uses the new binary.
  print(json.dumps({'ok':True,'binary_sha256':digest,'rollback':str(backup),'user_services_restarted':False,'existing_timers_preserved':True}))
 except BaseException:
  for name,existed in manifest['units'].items():
   if existed:shutil.copyfile(backup/name,UNITS/name)
   else:(UNITS/name).unlink(missing_ok=True)
  if manifest['previous_release']:
   temp=BASE/'rollback';temp.unlink(missing_ok=True);temp.symlink_to(manifest['previous_release']);temp.replace(current)
  else:current.unlink(missing_ok=True)
  system('daemon-reload')
  try:system('stop','cbgpu-rust-core.service')
  except Exception:pass
  raise
if __name__=='__main__':main()
