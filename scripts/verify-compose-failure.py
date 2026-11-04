#!/usr/bin/env python3
import importlib.util,json,pathlib,subprocess,tempfile,uuid,shutil,socket
root=pathlib.Path(__file__).resolve().parents[1];spec=importlib.util.spec_from_file_location('compose_worker',root/'deploy/compose-worker.py');worker=importlib.util.module_from_spec(spec);spec.loader.exec_module(worker)
id=str(uuid.uuid4());namespace='cbgpu-compose-'+id;fixture=pathlib.Path(tempfile.mkdtemp(prefix='cbgpu-failed-compose-'));secret='fixture-diagnostics-password-12345'
compose={'services':{'rest':{'image':'nginx:alpine','entrypoint':['/bin/sh','-c'],'command':['echo "PostgREST schema cache unavailable: $$DIAGNOSTIC_PASSWORD"; exec tail -f /dev/null'],'environment':{'DIAGNOSTIC_PASSWORD':secret},'healthcheck':{'test':['CMD-SHELL','echo "DB connection refused: $$DIAGNOSTIC_PASSWORD"; exit 1'],'interval':'1s','timeout':'1s','retries':1}},'db':{'image':'nginx:alpine','entrypoint':['/bin/sh','-c'],'command':['test -e /data/preserved || echo retained > /data/preserved; exec tail -f /dev/null'],'volumes':['db-data:/data']}} ,'volumes':{'db-data':{}}}
(fixture/'compose.yml').write_text(worker.yaml.safe_dump(compose));s=socket.socket();s.bind(('127.0.0.1',0));port=s.getsockname()[1];s.close();cfg={'id':id,'env_vars':{'CBGPU_GATEWAY_SERVICE':'rest'},'host_port':port,'container_port':8000,'public_url':'https://failure-test.server.cbgpu.com','memory_limit':'512m','cpu_limit':'0.5','access_scope':'public'}
def run(args):return subprocess.check_output(args,text=True).strip()
try:
 try:worker.deploy(cfg,fixture)
 except RuntimeError as error:assert 'rest' in str(error) and 'failed' in str(error)
 else:raise AssertionError('Unhealthy service was incorrectly marked successful')
 files=list((worker.BASE/id).glob('release-*/failure-diagnostics.json'));assert len(files)==1;raw=files[0].read_text();assert secret not in raw;d=json.loads(raw);assert 'rest' in d['failed_services'];assert 'schema cache unavailable' in d['container_logs'];assert '[REDACTED]' in d['container_logs'];assert 'rest' in d['compose_ps'];rest=next(x for x in d['services'] if x['service']=='rest');assert rest['health']=='unhealthy';assert 'DB connection refused' in rest['health_output'];assert secret not in rest['health_output'];assert run(['docker','inspect','--format','{{.State.Running}}',namespace+'-rest'])=='true';assert run(['docker','exec',namespace+'-db','cat','/data/preserved'])=='retained'
 print('PASS: unhealthy remains failure; service status, timestamped logs and health output persisted with secrets redacted. Both containers and persistent data remain available.')
finally:
 for container in run(['docker','ps','-a','--filter','label=com.docker.compose.project='+namespace,'--format','{{.ID}}']).split():subprocess.run(['docker','rm','-f',container],capture_output=True)
 for volume in run(['docker','volume','ls','--filter','label=com.docker.compose.project='+namespace,'--format','{{.Name}}']).split():
  if volume.startswith(namespace+'-'):subprocess.run(['docker','volume','rm',volume],capture_output=True)
 for network in run(['docker','network','ls','--filter','label=com.docker.compose.project='+namespace,'--format','{{.ID}}']).split():subprocess.run(['docker','network','rm',network],capture_output=True)
 shutil.rmtree(worker.BASE/id,ignore_errors=True);shutil.rmtree(fixture,ignore_errors=True);print('Only this temporary failure fixture was removed after preservation assertions.')
