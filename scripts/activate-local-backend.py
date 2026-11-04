"""Activate the already verified backend after final-sync.py succeeds."""
from pathlib import Path
import json,os,re,subprocess,time,urllib.request
root=Path(__file__).resolve().parents[1];repo=root.parent;private=root/'private';state=private/'cutover'
if not (state/'controller-paused').exists():raise SystemExit('GPU controller has not been paused.')
env={k:v for l in (root/'.env').read_text().splitlines() if l and not l.startswith('#') and '=' in l for k,v in [l.split('=',1)]}
config={k:v for l in (private/'frontend-runtime.env').read_text().splitlines() if '=' in l for k,v in [l.split('=',1)]}
frontend=repo/'FrontEnd/cbgpu';units=Path('/home/cbgpu/.config/systemd/user')
backup=state/'before';backup.mkdir(mode=0o700,exist_ok=True)
files=[frontend/'.env.local',units/'cbgpu-frontend.service',units/'cbgpu-ssh-gateway.service',units/'cbgpu-ssh-gateway-lan.service',Path('/home/cbgpu/CBGPU/Daemon/config.yaml'),Path('/home/cbgpu/.config/cbgpu/service_role_key')]
manifest=[]
for i,p in enumerate(files):
 data=p.read_bytes();target=backup/str(i);target.write_bytes(data);target.chmod(0o600);manifest.append({'path':str(p),'backup':str(target)})
(private/'activation-files.json').write_text(json.dumps(manifest));(private/'activation-files.json').chmod(0o600)
material=json.loads(subprocess.check_output(['docker','inspect','cbgpu-materials']))[0]
(private/'materials-before.json').write_text(json.dumps(material));(private/'materials-before.json').chmod(0o600)
def run(args,**kwargs):return subprocess.run(args,check=True,**kwargs)
def atomic(p,text,mode=0o600):
 temp=p.with_name(p.name+'.backend-next');temp.write_text(text);temp.chmod(mode);os.replace(temp,p)
try:
 p=frontend/'.env.local';s=p.read_text()
 for k,v in config.items():
  pattern='^'+re.escape(k)+'=.*$';s=re.sub(pattern,k+'='+v,s,flags=re.M) if re.search(pattern,s,re.M) else s+'\n'+k+'='+v+'\n'
 atomic(p,s)
 p=units/'cbgpu-frontend.service';s=p.read_text();s=re.sub(r'Environment=NEXT_BUILD_DIR=.*','Environment=NEXT_BUILD_DIR=.next-backend-classic',s)
 if 'EnvironmentFile=' not in s:s=s.replace('Environment=NODE_ENV=production','Environment=NODE_ENV=production\nEnvironmentFile='+str(private/'frontend-runtime.env'))
 atomic(p,s,0o644)
 for name in ['cbgpu-ssh-gateway.service','cbgpu-ssh-gateway-lan.service']:
  p=units/name;atomic(p,re.sub(r'Environment=CBGPU_SUPABASE_URL=.*','Environment=CBGPU_SUPABASE_URL=http://127.0.0.1:54321',p.read_text()),0o644)
 p=Path('/home/cbgpu/CBGPU/Daemon/config.yaml');s=p.read_text()
 for key,value in {'supabase_url':'http://127.0.0.1:54321','supabase_anon_key':env['ANON_KEY']}.items():s=re.sub('^'+key+':.*$',key+': '+json.dumps(value),s,flags=re.M)
 atomic(p,s,0o600)
 atomic(Path('/home/cbgpu/.config/cbgpu/service_role_key'),env['SERVICE_ROLE_KEY']+'\n')
 p=root/'.env';s=p.read_text().replace('DISABLE_SIGNUP=true','DISABLE_SIGNUP=false');atomic(p,s)
 with (private/'activation-compose.log').open('w') as log:run(['docker','compose','up','-d','--wait','auth','functions'],cwd=root,stdout=log,stderr=log)
 # Retain the previous file service for an immediate rollback.
 run(['docker','stop','cbgpu-materials'],stdout=subprocess.DEVNULL);run(['docker','rename','cbgpu-materials','cbgpu-materials-before-backend'])
 values=dict(v.split('=',1) for v in material['Config']['Env']);values.update({'CBGPU_SUPABASE_URL':'http://api-gw:8000','CBGPU_SUPABASE_ANON_KEY':env['ANON_KEY'],'CBGPU_MATERIALS_ORIGINS':'https://cbgpu.com,http://localhost:3000'})
 file=private/'materials-runtime.env';atomic(file,'\n'.join(k+'='+v for k,v in values.items())+'\n')
 command=['docker','run','-d','--name','cbgpu-materials','--restart','unless-stopped','--network','cbgpu-supabase_default','--env-file',str(file),'-p','127.0.0.1:8421:8421']
 for bind in material['HostConfig']['Binds'] or []:command+=['-v',bind]
 command+=[material['Config']['Image']]+material['Config']['Cmd'];run(command,stdout=subprocess.DEVNULL)
 # Publish the signed APK only after the local API is ready.
 apk=repo/'FrontEnd/cbgpu_app/build/app/outputs/flutter-apk/app-release.apk'
 previous=frontend/'public/downloads/cbgpu.apk';(backup/'previous.apk').write_bytes(previous.read_bytes())
 temp=previous.with_suffix('.apk.next');temp.write_bytes(apk.read_bytes());os.replace(temp,previous)
 (repo/'FrontEnd/cbgpu_app/store/cbgpu.apk').write_bytes(apk.read_bytes())
 run(['systemctl','--user','daemon-reload']);run(['systemctl','--user','restart','cbgpu-frontend.service','cbgpu-ssh-gateway.service','cbgpu-ssh-gateway-lan.service'])
 for _ in range(20):
  try:
   req=urllib.request.Request('http://127.0.0.1:3000/backend/auth/v1/health',headers={'apikey':env['SUPABASE_PUBLISHABLE_KEY']})
   with urllib.request.urlopen(req,timeout=3) as response:
    if response.status==200:break
  except Exception:time.sleep(1)
 else:raise RuntimeError('Public backend route did not become healthy')
 sql="SELECT cron.alter_job(jobid,active:=true) FROM cron.job WHERE jobname IN ('selfstudy-reminder-quarter-hour','refresh-study-supervisors');"
 run(['docker','exec','cbgpu-supabase-db','psql','-U','postgres','-v','ON_ERROR_STOP=1','-c',sql],stdout=subprocess.DEVNULL)
 (state/'backend-active').write_text('ready\n');print('Local backend activated; waiting for root controller restart.')
except Exception:
 for item in manifest:atomic(Path(item['path']),Path(item['backup']).read_text(),0o644 if item['path'].endswith('.service') else 0o600)
 names=subprocess.check_output(['docker','ps','-a','--format','{{.Names}}'],text=True).splitlines()
 if 'cbgpu-materials-before-backend' in names:
  if 'cbgpu-materials' in names:subprocess.run(['docker','rm','-f','cbgpu-materials'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  subprocess.run(['docker','rename','cbgpu-materials-before-backend','cbgpu-materials']);subprocess.run(['docker','start','cbgpu-materials'],stdout=subprocess.DEVNULL)
 if (backup/'previous.apk').exists():(frontend/'public/downloads/cbgpu.apk').write_bytes((backup/'previous.apk').read_bytes())
 subprocess.run(['systemctl','--user','daemon-reload']);subprocess.run(['systemctl','--user','restart','cbgpu-frontend.service','cbgpu-ssh-gateway.service','cbgpu-ssh-gateway-lan.service'])
 (state/'abort').write_text('activation failed\n');raise
