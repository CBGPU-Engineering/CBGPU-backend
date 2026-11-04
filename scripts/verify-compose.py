#!/usr/bin/env python3
import importlib.util,json,os,pathlib,shutil,socket,subprocess,tempfile,time,uuid,http.client
ROOT=pathlib.Path(__file__).resolve().parents[1];spec=importlib.util.spec_from_file_location('compose_worker',ROOT/'deploy/compose-worker.py');worker=importlib.util.module_from_spec(spec);spec.loader.exec_module(worker)
identifier=str(uuid.uuid4());namespace='cbgpu-compose-'+identifier;base=worker.BASE/identifier;fixture=pathlib.Path(tempfile.mkdtemp(prefix='cbgpu-compose-fixture-'));root=fixture/'server';root.mkdir();(root/'site').mkdir();(root/'web').mkdir();(root/'web/Dockerfile').write_text('FROM nginx:alpine\nRUN echo bounded-build > /cbgpu-build-check\n');(root/'site/index.html').write_text('<h1>CBGPU Compose fixture</h1>');(root/'volumes/db/data').mkdir(parents=True)
secret="fixture$literal'quote";doc={'services':{'api-gw':{'build':{'context':'./web'},'image':'compose-fixture-web','environment':{'CHECK_VALUE':'${CHECK_VALUE}','NGINX_ENTRYPOINT_WORKER_PROCESSES_AUTOTUNE':'1'},'volumes':['./site:/usr/share/nginx/html:ro']},'db':{'image':'nginx:alpine','entrypoint':['/bin/sh','-c'],'command':['n=$$(cat /data/counter 2>/dev/null || echo 0); n=$$((n+1)); echo $$n > /data/counter; exec tail -f /dev/null'],'environment':{'REVISION':'${REVISION}'},'volumes':['./volumes/db/data:/data']},'logs-agent':{'image':'nginx:alpine','entrypoint':['/bin/sh','-c'],'command':['exec tail -f /dev/null'],'volumes':['/var/run/docker.sock:/var/run/docker.sock:ro']}}}
(root/'compose.yml').write_text(worker.yaml.safe_dump(doc));port_socket=socket.socket();port_socket.bind(('127.0.0.1',0));port=port_socket.getsockname()[1];port_socket.close();cfg={'id':identifier,'git_branch':'main','env_vars':{'CBGPU_ROOT_DIRECTORY':'server','CBGPU_GATEWAY_SERVICE':'api-gw','CBGPU_STORAGE_GB':'40','CHECK_VALUE':secret,'REVISION':'1'},'host_port':port,'container_port':80,'public_url':'https://compose-test.server.cbgpu.com','memory_limit':'512m','cpu_limit':'0.5','access_scope':'public'}
socket_path=worker.BASE.parent/'compose-sockets'/(identifier+'.sock');socket_path.parent.mkdir(parents=True,exist_ok=True,mode=0o700);base.mkdir(parents=True,exist_ok=True);proxy=subprocess.Popen(['node',str(ROOT/'deploy/docker-log-proxy.mjs'),str(socket_path),namespace],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE);results=[]
def run(args):return subprocess.check_output(args,text=True).strip()
class UnixHTTP(http.client.HTTPConnection):
 def connect(self):self.sock=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);self.sock.connect(str(socket_path))
def request(method,path):
 conn=UnixHTTP('docker');conn.request(method,path);response=conn.getresponse();status=response.status;data=response.read();conn.close();return status,data
try:
 for _ in range(50):
  if socket_path.exists():break
  time.sleep(.1)
 for bad in [{'services':{'x':{'image':'alpine','privileged':True}}},{'services':{'x':{'image':'alpine','network_mode':'host'}}},{'services':{'x':{'image':'alpine','volumes':['/etc:/private:ro']}}},{'services':{'x':{'image':'alpine','env_file':'/etc/shadow'}}},{'services':{'x':{'image':'alpine','devices':['/dev/nvidia0']}}}]:
  try:worker.validate_raw(bad,root,cfg['env_vars'],base/'docker-log.sock')
  except ValueError:continue
  raise AssertionError('Unsafe Compose configuration was accepted')
 print('Privileged, host network, external mounts/env files and GPU device access rejected.')
 for revision in ['1','2']:
  cfg['env_vars']['REVISION']=revision;result=worker.deploy(cfg,fixture);results.append(result)
  assert run(['docker','exec',namespace+'-db','cat','/data/counter'])==revision
  assert run(['docker','exec',namespace+'-api-gw','printenv','CHECK_VALUE'])==secret
  managed=json.loads(pathlib.Path(result['file']).read_text());assert managed['services']['api-gw']['ports'][0]['host_ip']=='127.0.0.1';assert all(service['pids_limit']==256 for service in managed['services'].values())
 status,data=request('GET','/containers/json');assert status==200;containers=json.loads(data);assert len(containers)==3;assert all(c['Labels']['com.docker.compose.project']==namespace for c in containers)
 other=run(['docker','inspect','--format','{{.Id}}','cbgpu-supabase-db']);assert request('GET','/containers/'+other+'/json')[0]==403;assert request('POST','/containers/'+containers[0]['Id']+'/stop')[0]==403
 print('Actual Compose gateway, literal env values, resource limits, project-filtered read-only logs and redeployment data persistence passed.')
 run(['docker','compose','-p',namespace,'-f',results[-1]['file'],'down','--remove-orphans','--timeout','5']);assert all(run(['docker','volume','inspect','--format','{{.Name}}',volume])==volume for volume in results[-1]['volumes'])
 print('Stopping Compose preserved all data volumes.')
finally:
 if results:subprocess.run(['docker','compose','-p',namespace,'-f',results[-1]['file'],'down','--remove-orphans','--timeout','5'],capture_output=True,timeout=60)
 for container in run(['docker','ps','-a','--filter','label=com.docker.compose.project='+namespace,'--format','{{.ID}}']).split():run(['docker','rm','-f',container])
 for volume in run(['docker','volume','ls','--filter','label=com.docker.compose.project='+namespace,'--format','{{.Name}}']).split():
  if volume.startswith(namespace+'-'):subprocess.run(['docker','volume','rm',volume],capture_output=True)
 for network in run(['docker','network','ls','--filter','label=com.docker.compose.project='+namespace,'--format','{{.ID}}']).split():subprocess.run(['docker','network','rm',network],capture_output=True)
 proxy.terminate();proxy.wait(timeout=10)
 for result in results:
  for volume in result['volumes']:
   if volume.startswith(namespace+'-'):subprocess.run(['docker','volume','rm',volume],capture_output=True)
 subprocess.run(['docker','buildx','rm','cbgpu-compose-build-'+identifier[:12]],capture_output=True,env={**os.environ,'DOCKER_CONFIG':str(worker.BASE.parent/'compose-docker-client')})
 socket_path.unlink(missing_ok=True);shutil.rmtree(base,ignore_errors=True);shutil.rmtree(fixture,ignore_errors=True);print('Only temporary Compose fixtures were removed.')
