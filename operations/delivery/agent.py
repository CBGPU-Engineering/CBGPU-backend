#!/usr/bin/env python3
"""Outbound-only, CI-gated production delivery. Never runs pull-request code."""
import argparse,contextlib,fcntl,hashlib,json,os,re,shutil,signal,sqlite3,subprocess,sys,tempfile,time,urllib.request,uuid
from pathlib import Path
ROOT=Path(os.environ.get('CBGPU_DELIVERY_HOME',str(Path.home()/'.local/share/cbgpu-delivery')))
CONFIG=ROOT/'config.json'; STATE=ROOT/'state.json'; SETTINGS=Path.home()/'.config/systemd/user'
SHA=re.compile(r'^[0-9a-f]{40}$')
class Deferred(Exception):pass
class DeliveryError(Exception):pass

def atomic(path,data):
 path.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
 temp=path.with_name(path.name+'.tmp-'+uuid.uuid4().hex)
 with temp.open('x') as f:os.chmod(temp,0o600);f.write(data);f.flush();os.fsync(f.fileno())
 os.replace(temp,path)
def read(path,default=None):
 try:return json.loads(path.read_text())
 except FileNotFoundError:return default

def command(args,cwd=None,env=None,timeout=120):
 result=subprocess.run(args,cwd=cwd,env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=timeout)
 if result.returncode:raise DeliveryError('Command failed: '+str(args[0])+' '+str(args[1] if len(args)>1 else ''))
 return result.stdout.strip()
def github(route,method='GET',body=None):
 args=['gh','api',route,'--method',method]
 if body is not None:args+=['--input','-']
 p=subprocess.run(args,input=json.dumps(body) if body is not None else None,text=True,capture_output=True,timeout=45)
 if p.returncode:raise DeliveryError('GitHub API request failed (authentication, network or permissions).')
 return json.loads(p.stdout) if p.stdout.strip() else None

def candidate(repo):
 if not re.fullmatch(r'CBGPU-Engineering/CBGPU-(web|backend)',repo):raise DeliveryError('Repository is not allowed by installed policy.')
 sha=github('repos/'+repo+'/git/ref/heads/main')['object']['sha']
 if not SHA.fullmatch(sha):raise DeliveryError('Invalid main commit.')
 runs=github(f'repos/{repo}/actions/workflows/ci.yml/runs?branch=main&event=push&head_sha={sha}&per_page=5')['workflow_runs']
 successful=[r for r in runs if r['head_sha']==sha and r['head_branch']=='main' and r['event']=='push' and r['status']=='completed' and r['conclusion']=='success' and r['repository']['full_name']==repo]
 if not successful:raise Deferred('Waiting for successful main-branch CI on '+sha[:12])
 return sha,successful[0]['html_url']

def record(event,**extra):
 data={'at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'event':event,**extra}
 with (ROOT/'events.jsonl').open('a') as f:f.write(json.dumps(data)+'\n')
 print(json.dumps(data),flush=True)

def runtime_env(config):
 env=os.environ.copy()
 for line in Path(config['frontend_env']).read_text().splitlines():
  if line and not line.startswith('#') and '=' in line:
   key,value=line.split('=',1);env[key]=value
 return env

def build(args,directory,env,log):
 # Production values never appear in the persisted output, including failures.
 secrets=[v for k,v in env.items() if re.search(r'SECRET|PASSWORD|TOKEN|SERVICE_ROLE|RESEND|SMTP_PASS',k) and len(v)>8]
 with tempfile.TemporaryFile(mode='w+') as output:
  try:
   p=subprocess.run(args,cwd=directory,env=env,stdout=output,stderr=subprocess.STDOUT,timeout=1800)
  finally:
   output.seek(0);text=output.read()
   for secret in secrets:text=text.replace(secret,'[REDACTED]')
   with log.open('a') as target:target.write(text[-2_000_000:])
  if p.returncode:raise DeliveryError('Build or validation failed; inspect the private build log.')

def checkout(component,repo,sha):
 mirror=ROOT/'mirrors'/f'{component}.git';destination=ROOT/'releases'/component/sha/'backend' if component=='backend' else ROOT/'releases'/component/sha/'web'
 if destination.exists():
  if command(['git','rev-parse','HEAD'],cwd=destination)!=sha:raise DeliveryError('Release directory commit mismatch.')
  return destination
 mirror.parent.mkdir(parents=True,exist_ok=True)
 if not mirror.exists():command(['git','init','--bare',str(mirror)])
 command(['git','--git-dir',str(mirror),'fetch','--depth=1','https://github.com/'+repo+'.git',sha],timeout=180)
 destination.parent.mkdir(parents=True,exist_ok=True)
 command(['git','--git-dir',str(mirror),'worktree','add','--detach',str(destination),sha])
 return destination

def health(url,commit=None,host=None,timeout=45):
 deadline=time.monotonic()+timeout
 while time.monotonic()<deadline:
  try:
   req=urllib.request.Request(url,headers={'Host':host} if host else {})
   with urllib.request.urlopen(req,timeout=5) as response:
    body=response.read(100000)
    if response.status==200 and (not commit or json.loads(body).get('commit')==commit):return
  except Exception:pass
  time.sleep(1)
 raise DeliveryError('Health check failed for '+url)

@contextlib.contextmanager
def preview(args,env,url,commit=None,host=None):
 with tempfile.TemporaryFile() as output:
  process=subprocess.Popen(args,env=env,stdout=output,stderr=output,start_new_session=True)
  try:health(url,commit,host,90);yield
  finally:
   if process.poll() is None:
    os.killpg(process.pid,signal.SIGTERM)
    try:process.wait(timeout=10)
    except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()

def unit_update(original,directory,exec_start,environment=None):
 result=re.sub(r'^WorkingDirectory=.*$',f'WorkingDirectory={directory}',original,flags=re.M)
 result=re.sub(r'^ExecStart=.*$',f'ExecStart={exec_start}',result,flags=re.M)
 if environment:
  for k,v in environment.items():
   result=re.sub(r'^Environment='+re.escape(k)+r'=.*\n?','',result,flags=re.M)
   result=result.replace('[Service]','[Service]\nEnvironment='+k+'='+v,1)
 return result

def switch(units,checks):
 before={name:(SETTINGS/name).read_text() for name in units}
 atomic(ROOT/'transaction.json',json.dumps({'units':before,'checks':checks}))
 try:
  for name,text in units.items():atomic(SETTINGS/name,text)
  command(['systemctl','--user','daemon-reload'])
  for name in units:command(['systemctl','--user','restart',name],timeout=90)
  for check in checks:health(**check)
  (ROOT/'transaction.json').unlink(missing_ok=True)
 except Exception:
  for name,text in before.items():atomic(SETTINGS/name,text)
  command(['systemctl','--user','daemon-reload'])
  for name in units:command(['systemctl','--user','restart',name],timeout=90)
  # Rollback itself must be healthy; do not label an outage as recovered.
  for check in checks:
   recovery={k:v for k,v in check.items() if k!='commit'}
   if recovery['url'].endswith('/api/health'):recovery['url']=recovery['url'].removesuffix('api/health')
   if recovery['url']=='http://127.0.0.1:3510/health':
    command(['systemctl','--user','is-active','cbgpu-deploy.service'])
   else:health(**recovery)
  (ROOT/'transaction.json').unlink(missing_ok=True)
  raise
 return before

def deploy_web(config,sha,repo,log):
 directory=checkout('web',repo,sha);env=runtime_env(config);env.update(NEXT_BUILD_DIR='.next-production',CBGPU_RELEASE_SHA=sha,NEXT_TELEMETRY_DISABLED='1')
 apk=Path(config['legacy_web'])/'public/downloads/cbgpu.apk';target=directory/'public/downloads/cbgpu.apk'
 if apk.exists() and not target.exists():target.symlink_to(apk)
 if not (directory/'.delivery-built').exists():
  build(['npm','ci','--no-audit','--no-fund'],directory,env,log)
  build(['npm','run','build'],directory,env,log)
  atomic(directory/'.delivery-built',sha)
 # Preserve the exact build environment for restart; do not publish it.
 atomic(directory/'.env.production.local','\n'.join(k+'='+v for k,v in env.items() if (k.startswith('NEXT_PUBLIC_') or k in ['SUPABASE_SERVICE_ROLE_KEY','EDUSET_TOKEN_SECRET','RESEND_API_KEY','CBGPU_EMAIL_FROM','CBGPU_TRAFFIC_SECRET']))+'\n')
 args=['/usr/bin/node',str(directory/'node_modules/next/dist/bin/next'),'start',str(directory),'--hostname','127.0.0.1','--port','3002']
 with preview(args,env,'http://127.0.0.1:3002/api/health',sha):
  for host in ['cbgpu.com','app.cbgpu.com','docs.cbgpu.com','arxiv.cbgpu.com']:
   health('http://127.0.0.1:3002/',host=host,timeout=15)
 unit=(SETTINGS/'cbgpu-frontend.service').read_text()
 unit=re.sub(r'^EnvironmentFile=.*$',f'EnvironmentFile={directory}/.env.production.local',unit,flags=re.M)
 unit=unit_update(unit,directory,f'/usr/bin/node {directory}/node_modules/next/dist/bin/next start --hostname 127.0.0.1 --port 3000',{'NEXT_BUILD_DIR':'.next-production','CBGPU_RELEASE_SHA':sha})
 before=switch({'cbgpu-frontend.service':unit},[{'url':'http://127.0.0.1:3000/api/health','commit':sha}])
 return str(directory),before

def changed_paths(repo,previous,sha):
 if not previous:return []
 compare=github(f'repos/{repo}/compare/{previous}...{sha}')
 if compare.get('status') not in ['ahead','identical']:raise Deferred('Main history diverged; review deployment ancestry.')
 if len(compare.get('files',[]))>=300:raise Deferred('Change set is too large for unattended rollout.')
 return [x['filename'] for x in compare.get('files',[])]

def busy_backend(config):
 db=Path(config['runtime_backend'])/'private/frontend-deployments/deploy.sqlite'
 if not db.exists():return False
 with contextlib.closing(sqlite3.connect('file:'+str(db)+'?mode=ro',uri=True,timeout=5)) as connection:
  for sql in ["SELECT count(*) FROM deployments WHERE status IN ('queued','building')","SELECT count(*) FROM compose_states WHERE status='building'"]:
   try:
    if connection.execute(sql).fetchone()[0]:return True
   except sqlite3.OperationalError:pass
 return False

def backup_sqlite(config,sha):
 folder=ROOT/'snapshots'/sha;folder.mkdir(parents=True,exist_ok=True)
 for file in ['developer.sqlite','frontend-deployments/deploy.sqlite','careers/applications.sqlite']:
  path=Path(config['runtime_backend'])/'private'/file
  if not path.exists():continue
  target=folder/file;target.parent.mkdir(parents=True,exist_ok=True)
  with contextlib.closing(sqlite3.connect('file:'+str(path)+'?mode=ro',uri=True)) as source,contextlib.closing(sqlite3.connect(target)) as destination:source.backup(destination)

def deploy_backend(config,sha,repo,previous,log):
 paths=changed_paths(repo,previous,sha)
 protected=[p for p in paths if p.startswith(('schema/','volumes/','gpu/','latex/')) or p in ['docker-compose.yml','.gitmodules']]
 if protected:raise Deferred('Infrastructure/schema changes require a separate rollout: '+', '.join(protected[:8]))
 if busy_backend(config):raise Deferred('User deployment is running; waiting without interrupting its workers.')
 directory=checkout('backend',repo,sha);runtime=Path(config['runtime_backend'])
 for name in ['private','.env','functions.env','backups','releases']:
  source=runtime/name;target=directory/name
  if source.exists() and not target.exists():target.symlink_to(source)
 # The extracted backend keeps persistent runtime paths outside source control.
 parent=directory.parent
 for name,source in [('FrontEnd',Path(config['legacy_web']).parents[0]),('Daemon',Path(config['legacy_repo'])/'Daemon')]:
  if source.exists() and not (parent/name).exists():(parent/name).symlink_to(source)
 env=os.environ.copy();env['NEXT_TELEMETRY_DISABLED']='1'
 if not (directory/'developer/node_modules').exists():build(['npm','ci','--no-audit','--no-fund'],directory/'developer',env,log)
 build(['node','scripts/verify-mail-queue.mjs'],directory,env,log)
 build(['/usr/bin/python3','-m','unittest','discover','-s','careers','-p','test_*.py'],directory,env,log)
 if not (directory/'careers').exists():raise DeliveryError('Missing recruitment service.')
 # A preview must not send mail, expire real records or mutate production data.
 with tempfile.TemporaryDirectory(prefix='careers-preview-') as temporary:
  candidate_env={**env,'CAREERS_STATE_DIR':temporary,'CAREERS_ORIGIN':'https://careers.cbgpu.com'}
  candidate_env.pop('CAREERS_MAIL_ENV',None);candidate_env.pop('RESEND_API_KEY',None)
  with preview(['/usr/bin/python3',str(directory/'careers/server.py'),'--port','13530'],candidate_env,'http://127.0.0.1:13530/health'):pass
 if busy_backend(config):raise Deferred('A user deployment started during validation; rollout deferred.')
 backup_sqlite(config,sha)
 units={};checks=[]
 definitions=[('careers','cbgpu-careers.service','/usr/bin/python3 '+str(directory/'careers/server.py')+' --port 3530','http://127.0.0.1:3530/health'),('developer','cbgpu-developer-portal.service','/usr/bin/node '+str(directory/'developer/server.mjs'),'http://127.0.0.1:3400/health'),('deploy','cbgpu-deploy.service','/usr/bin/node '+str(directory/'deploy/server.mjs'),'http://127.0.0.1:3510/health')]
 for prefix,name,execution,url in definitions:
  if previous and not any(p.startswith(prefix+'/') for p in paths):continue
  original=(SETTINGS/name).read_text()
  extra={'CBGPU_RELEASE_SHA':sha,**({'CBGPU_PORTAL_ROOT':str(directory),'CBGPU_PORTAL_REPO':config['legacy_repo']} if prefix=='developer' else {})}
  units[name]=unit_update(original,directory if prefix!='deploy' else directory/'deploy',execution,extra)
  checks.append({'url':url,'commit':sha})
 before=switch(units,checks) if units else {}
 return str(directory),before

def job(config,event_id,component,sha,status,error=None):
 db=Path(config['runtime_backend'])/'private/developer.sqlite'
 if not db.exists():return
 with contextlib.closing(sqlite3.connect(db,timeout=10)) as c:
  if status=='running':c.execute("INSERT OR IGNORE INTO jobs(id,status,started_at,commit_sha,action) VALUES(?,?,datetime('now'),?,?)",(event_id,status,sha,'auto:'+component))
  else:c.execute("UPDATE jobs SET status=?,finished_at=datetime('now'),error=? WHERE id=?",(status,error,event_id))
  c.commit()

def notify(config,component,sha,status,event_id):
 # The worker retries delivery on subsequent polls. No credentials in this file.
 outbox=ROOT/'outbox';outbox.mkdir(exist_ok=True)
 atomic(outbox/(event_id+'.json'),json.dumps({'component':component,'commit':sha,'status':status,'id':event_id}))

def flush_notifications(config):
 values={line.split('=',1)[0]:line.split('=',1)[1] for line in (Path(config['runtime_backend'])/'.env').read_text().splitlines() if '=' in line and not line.startswith('#')}
 key=values.get('SMTP_PASS','')
 if not key.startswith('re_'):return
 for path in sorted((ROOT/'outbox').glob('*.json'))[:5]:
  item=read(path);subject=f"[CBGPU 운영] {item['component']} {item['status']} · {item['commit'][:12]}"
  body={'from':'CBGPU Operations <info@cbgpu.com>','to':['info@cbgpu.com'],'subject':subject,'html':f"<div style='font-family:Arial,sans-serif;padding:28px;max-width:560px'><h2>CBGPU 배포 상태</h2><p>{item['component']}: <b>{item['status']}</b></p><p>커밋: {item['commit'][:12]}</p><p>상세 로그는 개발자 페이지에서 확인해주세요.</p><a href='https://developers.cbgpu.com' style='background:#004fe8;color:white;padding:12px 18px;border-radius:8px;display:inline-block'>개발자 페이지 열기</a></div>"}
  try:
   request=urllib.request.Request('https://api.resend.com/emails',data=json.dumps(body).encode(),headers={'Authorization':'Bearer '+key,'Content-Type':'application/json','Idempotency-Key':'cbgpu-delivery-'+item['id']})
   with urllib.request.urlopen(request,timeout=15) as response:json.load(response)
   path.unlink()
  except Exception:break

def recover_transaction():
 journal=read(ROOT/'transaction.json')
 if not journal:return
 for name,text in journal['units'].items():atomic(SETTINGS/name,text)
 command(['systemctl','--user','daemon-reload'])
 for name in journal['units']:command(['systemctl','--user','restart',name],timeout=90)
 for check in journal['checks']:
  url=check['url']
  if url.endswith('/api/health'):url=url.removesuffix('api/health')
  if url=='http://127.0.0.1:3510/health':command(['systemctl','--user','is-active','cbgpu-deploy.service'])
  else:health(url)
 (ROOT/'transaction.json').unlink()
 record('interrupted_deployment_rolled_back')

def save(state):atomic(STATE,json.dumps(state,indent=2)+'\n')
def roll_back(config,state,component):
 current=state['components'].get(component,{})
 before=current.get('rollback_units')
 if not before:raise DeliveryError('No previous unit snapshot for '+component)
 checks=[{'url':'http://127.0.0.1:3000/'}] if component=='web' else [{'url':'http://127.0.0.1:3400/health'},{'url':'http://127.0.0.1:3530/health'}]
 switch(before,checks);current['blocked_sha']=current.get('sha');current['sha']=current.get('previous_sha');current['status']='rolled_back';current['rollback_units']=None;save(state);record('manual_rollback',component=component)

def poll(config,state,only=None):
 for component,repo in config['repositories'].items():
  if only and component!=only:continue
  current=state['components'].setdefault(component,{})
  try:
   sha,ci=candidate(repo)
   if current.get('sha')==sha or current.get('blocked_sha')==sha:continue
   if component=='backend' and current.get('sha'):
    paths=changed_paths(repo,current['sha'],sha)
    protected=[p for p in paths if p.startswith(('schema/','volumes/','gpu/','latex/')) or p in ['docker-compose.yml','.gitmodules']]
    if protected:raise Deferred('Infrastructure/schema changes require a separate rollout: '+', '.join(protected[:8]))
   if component=='backend' and busy_backend(config):raise Deferred('Waiting for active user deployment workers to finish.')
   if current.get('failed_sha')==sha:continue # Explicit retry is required after a failed deployment.
   if current.get('attempt_at',0)>time.time()-120:continue
   current['attempt_at']=int(time.time());current['status']='building';current['candidate']=sha;save(state)
   event_id=uuid.uuid4().hex;log=ROOT/'logs'/(event_id+'.log');log.parent.mkdir(exist_ok=True);log.touch(mode=0o600)
   portal_log=Path(config['runtime_backend'])/'private'/('deploy-'+event_id+'.log')
   if not portal_log.exists():portal_log.symlink_to(log)
   job(config,event_id,component,sha,'running');record('deploy_started',component=component,commit=sha,ci=ci,id=event_id)
   deployment=None
   try:
    deployment=github('repos/'+repo+'/deployments','POST',{'ref':sha,'environment':'production','auto_merge':False,'required_contexts':[],'production_environment':True,'description':'Host-verified main CI; immutable release and health-gated switch'})['id']
    github(f'repos/{repo}/deployments/{deployment}/statuses','POST',{'state':'in_progress','log_url':ci})
    result=deploy_web(config,sha,repo,log) if component=='web' else deploy_backend(config,sha,repo,current.get('sha'),log)
    current.update(previous_sha=current.get('sha'),sha=sha,release=result[0],rollback_units=result[1],status='healthy',deployed_at=int(time.time()),failed_sha=None,candidate=None)
    save(state);job(config,event_id,component,sha,'succeeded');record('deploy_succeeded',component=component,commit=sha,id=event_id);notify(config,component,sha,'배포 성공',event_id)
    if deployment:
     try:github(f'repos/{repo}/deployments/{deployment}/statuses','POST',{'state':'success','environment_url':'https://cbgpu.com' if component=='web' else 'https://developers.cbgpu.com','log_url':ci})
     except Exception:record('reporting_failed',component=component,commit=sha)
   except Deferred as error:
    current.update(status='deferred',message=str(error));save(state);job(config,event_id,component,sha,'deferred',str(error));record('deploy_deferred',component=component,reason=str(error))
    if deployment:github(f'repos/{repo}/deployments/{deployment}/statuses','POST',{'state':'inactive','description':'Waiting for safe operational rollout conditions.'})
   except Exception as error:
    current.update(status='failed',failed_sha=sha,message=str(error));save(state);job(config,event_id,component,sha,'failed',str(error));record('deploy_failed',component=component,error=str(error),commit=sha);notify(config,component,sha,'배포 실패 — 운영 상태 확인 필요',event_id)
    if deployment:
     try:github(f'repos/{repo}/deployments/{deployment}/statuses','POST',{'state':'failure','description':'Host rollout failed; inspect private delivery logs.'})
     except Exception:pass
  except Deferred as error:
   current['waiting_for']=str(error);save(state)
  except Exception as error:record('poll_failed',component=component,error=str(error))
 flush_notifications(config)
 state['last_poll']=int(time.time());save(state)

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--once',action='store_true');parser.add_argument('--status',action='store_true');parser.add_argument('--only',choices=['web','backend']);parser.add_argument('--retry',choices=['web','backend']);parser.add_argument('--rollback',choices=['web','backend']);parser.add_argument('--pause',action='store_true');parser.add_argument('--resume',action='store_true');args=parser.parse_args()
 os.umask(0o077);ROOT.mkdir(parents=True,exist_ok=True,mode=0o700)
 with (ROOT/'agent.lock').open('a') as lock:
  try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
  except BlockingIOError:print('Another delivery operation is running.');return
  config=read(CONFIG);state=read(STATE,{'components':{},'paused':False})
  if args.status:
   visible={**state,'components':{k:{x:y for x,y in v.items() if x!='rollback_units'} for k,v in state['components'].items()}};print(json.dumps(visible,indent=2));return
  if args.pause or args.resume:state['paused']=args.pause;save(state);record('paused' if args.pause else 'resumed');return
  if args.rollback:roll_back(config,state,args.rollback);return
  if args.retry:
   state['components'].setdefault(args.retry,{}).pop('failed_sha',None);state['components'][args.retry].pop('blocked_sha',None);state['components'][args.retry]['attempt_at']=0;save(state)
  if state.get('paused'):record('poll_skipped_paused');return
  recover_transaction()
  poll(config,state,args.only or args.retry)
if __name__=='__main__':main()
