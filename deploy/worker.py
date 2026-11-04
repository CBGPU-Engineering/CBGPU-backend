#!/usr/bin/env python3
"""Untrusted repository builds run only inside bounded, unprivileged containers."""
import json,os,pathlib,re,shutil,subprocess,sys,time,threading
cfg=json.load(sys.stdin);base=pathlib.Path(cfg['directory']);base.mkdir(parents=True,exist_ok=True);source=base/'source';container='cbfront-build-'+cfg['deployment_id'];runtime='cbfront-run-'+cfg['deployment_id'];started=False
secrets=[cfg.get('git_token','')]+list(cfg.get('env',{}).values())
def log(s):
 for secret in secrets:
  if secret and len(secret)>3:s=s.replace(secret,'[REDACTED]')
 print(s,flush=True)
def run(args,**kw):
 p=subprocess.run(args,capture_output=True,text=True,timeout=kw.pop('timeout',120),**kw)
 if p.returncode:raise RuntimeError((p.stderr or p.stdout or 'Command failed')[-3000:])
 return p.stdout.strip()
def safe(root,relative):
 value=(root/relative).resolve()
 if not value.is_relative_to(root.resolve()) or not value.is_dir():raise ValueError('Directory must stay inside this repository')
 return value
def size(root):
 total=0;count=0
 for d,dirs,files in os.walk(root,followlinks=False):
  for f in files:
   try:total+=os.lstat(os.path.join(d,f)).st_size;count+=1
   except OSError:pass
   if total>2*1024**3 or count>250000:return False
 return True
try:
 log('Cloning '+cfg['repository']+' (branch '+cfg['branch']+')')
 git_env={**os.environ,'GIT_TERMINAL_PROMPT':'0','GIT_CONFIG_COUNT':'1','GIT_CONFIG_KEY_0':'core.hooksPath','GIT_CONFIG_VALUE_0':'/dev/null'}
 if cfg.get('git_token'):
  import base64
  git_env.update(GIT_CONFIG_COUNT='2',GIT_CONFIG_KEY_1='http.https://github.com/.extraheader',GIT_CONFIG_VALUE_1='Authorization: Basic '+base64.b64encode(('x-access-token:'+cfg['git_token']).encode()).decode())
 run(['git','-c','protocol.file.allow=never','clone','--depth=1','--single-branch','--branch',cfg['branch'],'--',cfg['repository'],str(source)],env=git_env,timeout=90)
 commit=run(['git','-C',str(source),'rev-parse','HEAD']);shutil.rmtree(source/'.git');root=safe(source,cfg.get('root_directory') or '.')
 if not size(source):raise ValueError('Repository exceeds build storage allowance')
 package={}
 if (root/'package.json').is_file():package=json.loads((root/'package.json').read_text())
 dependencies={**package.get('dependencies',{}),**package.get('devDependencies',{})};framework=cfg['framework']
 if framework=='auto':framework='next' if 'next' in dependencies else 'vite' if 'vite' in dependencies else 'astro' if 'astro' in dependencies else 'static' if not package else 'other'
 output=cfg.get('output_directory') or ('dist' if framework in ('vite','astro') else 'build' if 'react-scripts' in dependencies else '.')
 install=cfg.get('install_command') or ('npm ci --no-audit --no-fund' if (root/'package-lock.json').is_file() else 'npm install --no-audit --no-fund')
 build=cfg.get('build_command') or ('npm run build' if package else '')
 if package or build:
  log('Framework: '+framework+' · Node.js 22');log('Installing dependencies and building…')
  command=(install+' && ' if package else '')+(build or 'true')
  args=['docker','run','--rm','--name',container,'--label','cbgpu.frontend.build=true','--user',str(os.getuid())+':'+str(os.getgid()),'--cpus','2','--memory','2g','--memory-swap','2g','--pids-limit','256','--cap-drop','ALL','--security-opt','no-new-privileges','--read-only','--tmpfs','/tmp:rw,nosuid,size=1g','--mount','type=bind,src='+str(source)+',dst=/workspace','-w','/workspace/'+str(root.relative_to(source)),'--env','HOME=/tmp','--env','npm_config_cache=/tmp/npm','--env','NEXT_TELEMETRY_DISABLED=1']
  for k,v in cfg.get('env',{}).items():args+=['--env',k+'='+v]
  args+=['node:22-bookworm-slim','/bin/sh','-lc',command]
  proc=subprocess.Popen(args,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  def output_logs():
   for line in proc.stdout:log(line.rstrip()[:6000])
  reader=threading.Thread(target=output_logs,daemon=True);reader.start();deadline=time.monotonic()+600
  while proc.poll() is None:
   if time.monotonic()>deadline or not size(source):
    subprocess.run(['docker','rm','-f',container],capture_output=True);proc.wait(timeout=20);raise RuntimeError('Build exceeded the 10-minute or 2 GB workspace limit')
   time.sleep(2)
  reader.join(timeout=5)
  if proc.returncode:raise RuntimeError('Build failed. Check the build output above.')
 next_runtime=framework=='next' and (root/'.next'/'BUILD_ID').is_file() and not (root/'out'/'index.html').is_file()
 if next_runtime:
  log('Starting isolated Next.js runtime…')
  args=['docker','run','-d','--name',runtime,'--label','cbgpu.frontend.runtime=true','--restart','unless-stopped','--user',str(os.getuid())+':'+str(os.getgid()),'--cpus','1','--memory','768m','--memory-swap','768m','--pids-limit','128','--cap-drop','ALL','--security-opt','no-new-privileges','--read-only','--tmpfs','/tmp:rw,nosuid,size=256m','--mount','type=bind,src='+str(source)+',dst=/workspace,readonly','-w','/workspace/'+str(root.relative_to(source)),'-p','127.0.0.1::3000','--env','NODE_ENV=production','--env','NEXT_TELEMETRY_DISABLED=1','--log-opt','max-size=5m','--log-opt','max-file=2']
  for k,v in cfg.get('env',{}).items():args+=['--env',k+'='+v]
  args+=['--tmpfs','/workspace/'+str(root.relative_to(source))+'/.next/cache:rw,nosuid,uid='+str(os.getuid())+',gid='+str(os.getgid())+',size=128m']
  args+=['node:22-bookworm-slim','node','node_modules/next/dist/bin/next','start','--hostname','0.0.0.0','--port','3000']
  run(args);started=True;port=int(json.loads(run(['docker','inspect',runtime]))[0]['NetworkSettings']['Ports']['3000/tcp'][0]['HostPort'])
  import urllib.request
  for attempt in range(60):
   try:
    with urllib.request.urlopen('http://127.0.0.1:'+str(port)+'/',timeout=2) as response:
     if response.status<500:break
   except Exception:time.sleep(1)
  else:raise RuntimeError('Runtime did not pass its startup health check')
  result={'kind':'next','port':port,'container':runtime,'directory':str(base),'commit':commit,'framework':framework}
 else:
  if framework=='next' and (root/'out'/'index.html').is_file():output='out'
  public=safe(root,output);artifact=base/'public';artifact.mkdir(exist_ok=True);total=0
  if public==root and package:raise ValueError('Set an output directory such as dist, build or out; source directories cannot be published')
  for current,dirs,files in os.walk(public,followlinks=False):
   dirs[:]=[d for d in dirs if not d.startswith('.') and d not in ('node_modules',)]
   for name in files:
    file=pathlib.Path(current)/name
    if file.is_symlink() or name.startswith('.') or name in ('package.json','package-lock.json') or name.endswith(('.env','.pem','.key')):continue
    total+=file.stat().st_size
    if total>250*1024**2:raise ValueError('Published output exceeds 250 MB')
    target=artifact/file.relative_to(public);target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(file,target)
  if not (artifact/'index.html').is_file():raise ValueError('Output directory must contain index.html')
  shutil.rmtree(source);result={'kind':'static','public':str(artifact),'directory':str(base),'commit':commit,'framework':framework,'spa':framework in ('vite','other')}
 log('Deployment passed. Promoting to production.');print('CBGPU_RESULT '+json.dumps(result),flush=True)
except Exception as e:
 if started:subprocess.run(['docker','rm','-f',runtime],capture_output=True)
 subprocess.run(['docker','rm','-f',container],capture_output=True);log('ERROR: '+str(e));sys.exit(1)
