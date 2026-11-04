#!/usr/bin/env python3
import importlib.util,json,pathlib,socket,subprocess,tempfile,time,uuid,shutil,urllib.request
ROOT=pathlib.Path(__file__).resolve().parents[2];spec=importlib.util.spec_from_file_location('daemon_runtime',ROOT/'Daemon/daemon.py');d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d)
fixture=pathlib.Path(tempfile.mkdtemp(prefix='cbgpu-server-root-'));(fixture/'backend api').mkdir();(fixture/'backend api/sentinel').write_text('correct-directory');(fixture/'escape').symlink_to('/etc',target_is_directory=True)
def run(args,**kw):return subprocess.run(args,capture_output=True,text=True,timeout=kw.pop('timeout',120),**kw)
try:
 _,_,setup=d.cpu_git_configuration({'runtime':'python311','env_vars':{'CBGPU_RUNTIME_PRESET':'python312','CBGPU_ROOT_DIRECTORY':'backend api'}})
 result=run(['docker','run','--rm','--network','none','--mount','type=bind,src='+str(fixture)+',dst=/app,readonly','python:3.12-slim','sh','-c','set -e; '+setup+'cat sentinel']);assert result.returncode==0 and result.stdout=='correct-directory'
 _,_,setup=d.cpu_git_configuration({'runtime':'python311','env_vars':{'CBGPU_ROOT_DIRECTORY':'escape'}});result=run(['docker','run','--rm','--network','none','--mount','type=bind,src='+str(fixture)+',dst=/app,readonly','python:3.12-slim','sh','-c','set -e; '+setup+'exit 0']);assert result.returncode!=0
 for root in ['/etc','../outside','x/../outside','folder\\other']:
  try:d.cpu_git_configuration({'runtime':'node20','env_vars':{'CBGPU_ROOT_DIRECTORY':root}})
  except ValueError:continue
  raise AssertionError('Unsafe root accepted')
 print('Nested repository roots, spaces, traversal rejection and actual symlink-escape blocking passed.')
finally:shutil.rmtree(fixture,ignore_errors=True)
identifier=str(uuid.uuid4());sock=socket.socket();sock.bind(('127.0.0.1',0));port=sock.getsockname()[1];sock.close();project={'id':identifier,'host_port':port,'container_port':8080,'access_scope':'private','cpu_limit':'2','memory_limit':'2g','deploy_type':'git','git_url':'https://github.com/spring-guides/gs-rest-service','git_branch':'main','runtime':'node20','run_command':'mvn -B -DskipTests package && java -jar target/*.jar','env_vars':{'CBGPU_RUNTIME_PRESET':'java21','CBGPU_ROOT_DIRECTORY':'complete'}}
try:
 d.start_cpu_container(project,{})
 address=d.private_bind_address()
 for attempt in range(150):
  try:
   with urllib.request.urlopen('http://'+address+':'+str(port)+'/greeting',timeout=3) as response:data=json.load(response)
   if data.get('content','').startswith('Hello'):break
  except Exception:time.sleep(2)
 else:raise RuntimeError('Spring Boot did not answer its greeting endpoint')
 print('Real Spring Boot Git deployment with Java 21/Maven from complete/ returned its HTTP API successfully.')
finally:
 run(['docker','rm','-f',d.cpu_container_name(identifier)]);run(['docker','volume','rm',d.cpu_volume_name(identifier)]);print('Temporary Spring Boot container and its test-only volume removed.')
