#!/usr/bin/env python3
"""Validate and deploy an isolated Compose project; never remove persistent volumes."""
import json,os,pathlib,re,shutil,subprocess,sys,time,yaml,hashlib,threading,resource
os.umask(0o077)
BASE=pathlib.Path(__file__).resolve().parents[1]/'private'/'compose-deployments'
DANGEROUS=('privileged','devices','device_cgroup_rules','gpus','volumes_from','pid','ipc','userns_mode','cgroup_parent','cgroup','uts','use_api_socket','post_start','pre_stop','develop','provider','credential_spec')
def command(args,**kwargs):
 result=subprocess.run(args,capture_output=True,text=True,timeout=kwargs.pop('timeout',120),**kwargs)
 if result.returncode:raise RuntimeError((result.stderr or result.stdout or 'Command failed')[-6000:])
 return result.stdout

def within(root,path):
 value=(root/path).resolve() if not pathlib.Path(path).is_absolute() else pathlib.Path(path).resolve()
 if not value.is_relative_to(root.resolve()):raise ValueError('Compose files and build contexts must stay inside the repository')
 return value

def expand(value,env):
 def replacement(match):return env.get(match[1],match[3] or '')
 value=re.sub(r'\$\{([A-Za-z_]\w*)(?:(:-|-)([^}]*))?\}',replacement,str(value))
 if '$' in value:raise ValueError('Unresolved variable in a filesystem path')
 return value

def validate_raw(raw,root,env,socket_path):
 if not isinstance(raw,dict) or raw.get('include'):raise ValueError('Compose include is not supported')
 services=raw.get('services')
 if not isinstance(services,dict) or not 1<=len(services)<=20:raise ValueError('Use 1–20 Compose services')
 for name,service in services.items():
  if not re.fullmatch(r'[a-zA-Z0-9][a-zA-Z0-9_-]{0,60}',name) or not isinstance(service,dict):raise ValueError('Invalid service name')
  for key in DANGEROUS:
   if service.get(key):raise ValueError('Unsafe Compose setting: '+key)
  if service.get('extends') or service.get('external_links') or service.get('extra_hosts'):raise ValueError('External host/service access settings are not supported')
  if service.get('network_mode'):raise ValueError('Host/container network modes are not allowed')
  if service.get('runtime') not in (None,'runc'):raise ValueError('Custom/GPU runtimes are not allowed')
  if service.get('sysctls'):raise ValueError('Kernel sysctls are not allowed')
  if any(str(cap).upper() not in ('SYS_NICE','NET_BIND_SERVICE') for cap in service.get('cap_add',[])):raise ValueError('Requested container capabilities are not allowed')
  if any(str(opt) not in ('no-new-privileges:true','no-new-privileges=true','label=disable') for opt in service.get('security_opt',[])):raise ValueError('Unsafe security option')
  files=service.get('env_file') or [];files=[files] if isinstance(files,(str,dict)) else files
  for file in files:within(root,expand(file.get('path') if isinstance(file,dict) else file,env))
  build=service.get('build')
  if build:
   build={'context':build} if isinstance(build,str) else build
   if not isinstance(build,dict) or any(key not in ('context','dockerfile','dockerfile_inline','args','target') for key in build) or build.get('privileged') or build.get('entitlements') or build.get('ssh') or build.get('additional_contexts') or build.get('network'):raise ValueError('Unsafe build configuration')
   context=within(root,expand(build.get('context','.'),env));within(context,expand(build.get('dockerfile','Dockerfile'),env))
  volumes=[]
  for item in service.get('volumes') or []:
   if isinstance(item,str):
    parts=item.split(':');source=parts[0] if len(parts)>1 else None;target=parts[1] if len(parts)>1 else parts[0];mode=parts[2] if len(parts)>2 else '';item={'type':'bind' if source and (source.startswith(('.','/')) or '$' in source) else 'volume','source':source,'target':target,'read_only':'ro' in mode.split(',')}
   else:item=dict(item)
   target=str(item.get('target',''))
   if not target.startswith('/') or target=='/' or any(target==prefix or target.startswith(prefix+'/') for prefix in ('/proc','/sys','/dev')):raise ValueError('Unsafe container mount target')
   if target in ('/var/run/docker.sock','/run/docker.sock'):
    item={'type':'bind','source':str(socket_path),'target':target,'read_only':True}
   elif item.get('type')=='bind':
    item['source']=str(within(root,expand(item.get('source',''),env)))
   elif item.get('type') not in ('volume','tmpfs'):raise ValueError('Unsupported volume driver')
   volumes.append(item)
  service['volumes']=volumes
 for section in ('configs','secrets'):
  for value in (raw.get(section) or {}).values():
   if value.get('external'):raise ValueError('External configs/secrets are not allowed')
   if value.get('file'):within(root,expand(value['file'],env))
 for section in ('volumes','networks'):
  for value in (raw.get(section) or {}).values():
   value=value or {}
   if value.get('external') or value.get('driver_opts') or value.get('ipam'):raise ValueError('External/custom storage and network drivers are not allowed')
   if value.get('driver') not in (None,'local' if section=='volumes' else 'bridge'):raise ValueError('Unsafe storage/network driver')
 return raw

def escape(value):
 if isinstance(value,str):return value.replace('$','$$')
 if isinstance(value,list):return[escape(item) for item in value]
 if isinstance(value,dict):return{key:escape(item) for key,item in value.items()}
 return value

def prepare(cfg,fixture_source=None):
 namespace='cbgpu-compose-'+cfg['id'];base=BASE/cfg['id'];release=base/('release-'+str(time.time_ns()));source=release/'source';release.mkdir(parents=True,exist_ok=True)
 if fixture_source:shutil.copytree(fixture_source,source)
 else:command(['git','-c','protocol.file.allow=never','-c','core.hooksPath=/dev/null','clone','--depth=1','--single-branch','--branch',cfg.get('git_branch') or 'main','--',cfg['git_url'],str(source)],timeout=120,env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':str(release),'GIT_TERMINAL_PROMPT':'0'})
 values={str(k):str(v) for k,v in (cfg.get('env_vars') or {}).items()};root=within(source,values.get('CBGPU_ROOT_DIRECTORY') or '.')
 if not root.is_dir():raise ValueError('Root Directory does not exist')
 compose_name=values.get('CBGPU_COMPOSE_FILE')
 if compose_name:file=within(root,compose_name)
 else:file=next((root/name for name in ('compose.yaml','compose.yml','docker-compose.yml','docker-compose.yaml') if(root/name).is_file()),None)
 if not file or not file.is_file():raise ValueError('Compose file not found in Root Directory')
 dotenv=within(root,'.env');original=dotenv.read_text() if dotenv.is_file() else ''
 for line in original.splitlines():
  if '=' in line and not line.lstrip().startswith('#'):
   key,value=line.split('=',1);values.setdefault(key.strip(),value.strip().strip('"').strip("'"))
 values.setdefault('CBGPU_PUBLIC_URL',cfg['public_url']);values.setdefault('API_EXTERNAL_URL',cfg['public_url']);values.setdefault('SUPABASE_PUBLIC_URL',cfg['public_url'])
 if any('\n' in value or '\r' in value or '\0' in value for value in values.values()):raise ValueError('Compose environment values must be single-line')
 dotenv.write_text('\n'.join(key+"='"+value.replace("'","\\'")+"'" for key,value in values.items())+'\n');dotenv.chmod(0o600)
 socket=BASE.parent/'compose-sockets'/(cfg['id']+'.sock');socket.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
 raw=validate_raw(yaml.safe_load(file.read_text()),root,values,socket)
 raw['name']=namespace
 for name,service in raw['services'].items():service['container_name']=namespace+'-'+name
 intermediate=root/'.cbgpu-compose-input.yml';intermediate.write_text(yaml.safe_dump(raw));intermediate.chmod(0o600)
 normalized=json.loads(command(['docker','compose','--env-file',str(dotenv),'-p',namespace,'-f',str(intermediate),'config','--format','json'],cwd=root,timeout=30,env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':str(release)}))
 services=normalized['services'];gateway=values.get('CBGPU_GATEWAY_SERVICE')
 if not gateway:gateway=next((key for key in ('api-gw','kong','web','app') if key in services),None) or next(iter(services))
 if gateway not in services:raise ValueError('Gateway service was not found: '+str(gateway))
 weights={name:8 if name in ('db','postgres','database') else 5 if name in ('analytics','logflare') else 3 if name in ('functions','studio',gateway) else 2 if name in ('realtime','supavisor','storage') else 1 for name in services};total=sum(weights.values());memory=str(cfg.get('memory_limit','4g')).lower();memory_bytes=int(float(memory[:-1])*(1024**3 if memory[-1]=='g' else 1024**2));cpu=float(cfg.get('cpu_limit','3'));volumes=normalized.setdefault('volumes',{});persistent=[]
 for name,value in volumes.items():value['name']=namespace+'-'+name;value.pop('external',None);persistent.append(value['name'])
 for name,service in services.items():
  service['ports']=[];service['stop_grace_period']='20s';environment=service.setdefault('environment',{});image=str(service.get('image',''))
  if image.startswith('nginx:') or '/nginx:' in image:environment.setdefault('NGINX_ENTRYPOINT_WORKER_PROCESSES_AUTOTUNE','1')
  if any(label in image for label in ('supabase/realtime','supabase/logflare','supabase/supavisor')):
   environment['ERL_FLAGS']=(str(environment.get('ERL_FLAGS',''))+' +S 2:2 +SDcpu 1:1 +SDio 1').strip()

  if service.get('build'):service['image']=namespace+'-'+name+':build'
  elif str(service.get('image','')).startswith(('cbgpu/','cbgpu-','cbcpu_','jupiterhub_')):raise ValueError('Infrastructure images are reserved')
  service['cpus']=round(cpu*weights[name]/total,4);service['mem_limit']=int(memory_bytes*weights[name]/total);service['memswap_limit']=service['mem_limit'];service['pids_limit']=256;service['security_opt']=['no-new-privileges:true'];service.pop('deploy',None);service.pop('oom_score_adj',None);service['oom_kill_disable']=False;service['logging']={'driver':'json-file','options':{'max-size':'10m','max-file':'2'}}
  for volume in service.get('volumes',[]):
   if volume.get('type')=='bind':
    src=pathlib.Path(volume['source']);target=volume['target']
    if target in ('/var/run/docker.sock','/run/docker.sock'):volume['source']=str(socket);volume['read_only']=True;continue
    within(source,src)
    data=target in ('/var/lib/postgresql/data','/var/lib/storage','/data','/var/lib/mysql','/var/lib/redis') or str(src).endswith(('/volumes/db/data','/volumes/storage'))
    if data:
     key='persist-'+hashlib.sha256((name+':'+target).encode()).hexdigest()[:12];volume.clear();volume.update(type='volume',source=key,target=target);volumes[key]={'name':namespace+'-'+key};persistent.append(namespace+'-'+key)
    else:
     if not src.exists():raise ValueError('Missing configuration mount: '+str(src.relative_to(source)))
     volume['read_only']=True
 service=services[gateway];addresses=['127.0.0.1'];scope=cfg.get('access_scope','public')
 if scope=='private':addresses.append(cfg.get('private_host','100.100.136.37'))
 elif scope=='school':addresses.append(cfg.get('school_host','10.138.23.12'))
 service['ports']=[{'target':int(cfg.get('container_port') or 8000),'published':str(cfg['host_port']),'host_ip':address,'protocol':'tcp','mode':'ingress'} for address in addresses]
 for name,network in normalized.get('networks',{}).items():network['name']=namespace+'-'+name
 normalized['name']=namespace;managed=release/'compose-managed.json';managed.write_text(json.dumps(normalized));managed.chmod(0o600)
 return{'namespace':namespace,'file':str(managed),'release':str(release),'gateway':gateway,'volumes':list(set(persistent)),'services':list(services),'socket':str(socket),'storage_gb':int(values.get('CBGPU_STORAGE_GB','40'))},values

def collect_failure_diagnostics(result,secrets):
 """Read only this Compose project's containers; retain sanitized evidence."""
 import tempfile,urllib.parse
 deadline=time.monotonic()+90
 def redact(text):
  for secret in sorted(set(secrets),key=len,reverse=True):
   for value in (secret,urllib.parse.quote(secret,safe='')):text=text.replace(value,'[REDACTED]')
  text=re.sub(r'\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b','[REDACTED]',text)
  return re.sub(r'(https?://|postgres(?:ql)?://)([^\s/@]+):([^\s/@]+)@',r'\1[REDACTED]@',text)
 def capture(args,limit=20000):
  remaining=deadline-time.monotonic()
  if remaining<=0:return '[diagnostic collection deadline reached]',124
  with tempfile.TemporaryFile() as output:
   try:
    completed=subprocess.run(args,stdout=output,stderr=subprocess.STDOUT,text=True,timeout=min(30,remaining))
    code=completed.returncode
   except subprocess.TimeoutExpired:code=124
   output.seek(0,2);length=output.tell();output.seek(max(0,length-limit));text=output.read().decode('utf-8',errors='replace')
   return redact(('[earlier output truncated]\n' if length>limit else '')+text),code
 compose=['docker','compose','-p',result['namespace'],'-f',result['file']]
 ps,_=capture(compose+['ps','-a']);logs,_=capture(compose+['logs','--no-color','--timestamps','--tail=200'],100000)
 ids,_=capture(['docker','ps','-a','--filter','label=com.docker.compose.project='+result['namespace'],'--format','{{.ID}}'])
 services=[];failed=[];health_budget=60000
 for identifier in ids.splitlines()[:20]:
  if not re.fullmatch('[a-f0-9]{12,64}',identifier):continue
  raw,code=capture(['docker','inspect','--format','{{json .State}}',identifier],18000)
  identity,_=capture(['docker','inspect','--format','{{index .Config.Labels "com.docker.compose.service"}}',identifier],200)
  name=identity.strip()
  try:state=json.loads(raw) if code==0 else {}
  except ValueError:state={}
  status=state.get('Status','unknown');health=(state.get('Health') or {}).get('Status');bad=health in ('unhealthy','starting') or status not in ('running',) and not(status=='exited' and state.get('ExitCode')==0)
  entry={'service':name,'container':identifier,'status':status,'health':health,'exit_code':state.get('ExitCode'),'oom_killed':state.get('OOMKilled',False),'error':state.get('Error','')}
  if bad:
   failed.append(name or identifier)
   history,_=capture(['docker','inspect','--format','{{json .State.Health.Log}}',identifier],min(16000,max(1000,health_budget)))
   health_budget-=len(history);entry['health_output']=history if health_budget>=0 else '[health output truncated]'
  services.append(entry)
 for name in result.get('services',[]):
  if name not in {entry['service'] for entry in services}:
   failed.append(name);services.append({'service':name,'container':'not-created','status':'missing','health':None,'exit_code':None,'oom_killed':False,'error':'Container was not created or could not be inspected','health_output':'No health-check history available.'})
 diagnostics={'failed_services':failed,'services':services,'compose_ps':ps,'container_logs':logs,'captured_at':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'containers_preserved':True,'volumes_preserved':True}
 path=pathlib.Path(result['release'])/'failure-diagnostics.json';path.write_text(json.dumps(diagnostics,ensure_ascii=False));path.chmod(0o600)
 print('=== Compose failure diagnostics: '+(', '.join(failed) or 'startup/readiness failure')+' ===',flush=True)
 print('$ docker compose ps -a\n'+ps,flush=True)
 print('$ docker compose logs --no-color --timestamps --tail=200\n'+logs,flush=True)
 for entry in services:
  if entry['service'] in failed or entry['container'] in failed:
   print('=== Failed service: '+entry['service']+' | state='+entry['status']+' | health='+str(entry['health'])+' | exit='+str(entry['exit_code'])+' | OOMKilled='+str(entry['oom_killed'])+' ===',flush=True)
   print('$ docker inspect --format \'{{json .State.Health.Log}}\' '+entry['container']+'\n'+entry.get('health_output',''),flush=True)
 print('CBGPU_DIAGNOSTICS '+json.dumps(diagnostics,ensure_ascii=False),flush=True)
 return diagnostics

def deploy(cfg,fixture_source=None):
 result,values=prepare(cfg,fixture_source)
 print('CBGPU_ATTEMPT '+json.dumps(result),flush=True)
 secrets=[v for k,v in values.items() if len(v)>3 and k not in ('CBGPU_ROOT_DIRECTORY','CBGPU_GATEWAY_SERVICE','CBGPU_STORAGE_GB','CBGPU_COMPOSE_FILE','CBGPU_DEPLOY_ENGINE','CBGPU_RUNTIME_PRESET')]
 builder='cbgpu-compose-build-'+cfg['id'][:12]
 managed=json.loads(pathlib.Path(result['file']).read_text());secrets.extend(str(v) for service in managed['services'].values() for k,v in (service.get('environment') or {}).items() if re.search('SECRET|TOKEN|PASSWORD|PASSWD|KEY|DATABASE_URL|DB_URL',k,re.I) and isinstance(v,str) and len(v)>3);has_build=any(service.get('build') for service in managed['services'].values())
 if has_build:
  client=BASE.parent/'compose-docker-client';client.mkdir(parents=True,exist_ok=True,mode=0o700);build_env={'PATH':os.environ.get('PATH','/usr/bin:/bin'),'HOME':str(client),'DOCKER_CONFIG':str(client)}
  exists=subprocess.run(['docker','buildx','inspect',builder],capture_output=True,env=build_env).returncode==0
  if not exists:command(['docker','buildx','create','--name',builder,'--driver','docker-container','--driver-opt','memory=2g,memory-swap=2g,cpu-quota=200000,cpu-period=100000,default-load=true'],env=build_env,timeout=30)
  try:
   built=subprocess.run(['docker','compose','-p',result['namespace'],'-f',result['file'],'build','--builder',builder],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,env=build_env,timeout=900)
   output=built.stdout[-150000:]
   for secret in secrets:output=output.replace(secret,'[REDACTED]')
   print(output,flush=True)
   if built.returncode:raise RuntimeError('Compose image build failed')
  finally:subprocess.run(['docker','buildx','stop',builder],capture_output=True,env=build_env,timeout=30)
 process=subprocess.Popen(['docker','compose','-p',result['namespace'],'-f',result['file'],'up','-d','--no-build','--wait','--wait-timeout','300','--remove-orphans'],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
 def output():
  for line in process.stdout:
   for secret in secrets:line=line.replace(secret,'[REDACTED]')
   print(line.rstrip()[:6000],flush=True)
 reader=threading.Thread(target=output,daemon=True);reader.start();deadline=time.monotonic()+900
 timed_out=False
 while process.poll() is None:
  if time.monotonic()>deadline:
   timed_out=True;process.kill();process.wait();break
  time.sleep(1)
 reader.join(timeout=5)
 if timed_out or process.wait()!=0:
  try:diagnostics=collect_failure_diagnostics(result,secrets)
  except Exception as error:
   print('Failure diagnostics collection error: '+str(error),flush=True);diagnostics={}
  failed=', '.join(diagnostics.get('failed_services',[])) or 'startup/readiness check'
  raise RuntimeError('Compose deployment failed: '+failed+'. Containers and persistent volumes were retained for diagnosis.')
 return result
if __name__=='__main__':
 try:
  cfg=json.load(sys.stdin);result=deploy(cfg);print('CBGPU_RESULT '+json.dumps(result),flush=True)
 except Exception as e:print('ERROR: '+str(e),flush=True);sys.exit(1)
