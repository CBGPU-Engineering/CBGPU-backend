#!/usr/bin/env python3
"""Read-only snapshot of an existing failed project's containers into its deployment log."""
import contextlib,datetime,importlib.util,io,json,pathlib,re,sqlite3,sys,uuid
ROOT=pathlib.Path(__file__).resolve().parents[1];identifier=str(uuid.UUID(sys.argv[1]));spec=importlib.util.spec_from_file_location('compose_worker',ROOT/'deploy/compose-worker.py');worker=importlib.util.module_from_spec(spec);spec.loader.exec_module(worker)
base=worker.BASE/identifier;db=sqlite3.connect(ROOT/'private/frontend-deployments/deploy.sqlite');db.row_factory=sqlite3.Row;state=db.execute('SELECT * FROM compose_states WHERE project_id=?',(identifier,)).fetchone()
if not state or state['status']!='error':raise SystemExit('Only an existing failed deployment can be diagnosed; no containers were changed.')
files=sorted(base.glob('release-*/compose-managed.json'),reverse=True)
if not files:raise SystemExit('No managed Compose configuration found.')
file=files[0];managed=json.loads(file.read_text());services=managed['services'];gateway=next((name for name,service in services.items() if service.get('ports')),next(iter(services)));volumes=[v['name'] for v in managed.get('volumes',{}).values() if v.get('name')];attempt={'namespace':'cbgpu-compose-'+identifier,'file':str(file),'release':str(file.parent),'services':list(services),'gateway':gateway,'volumes':volumes,'storage_gb':state['storage_gb']}
secrets=[str(value) for service in services.values() for key,value in (service.get('environment') or {}).items() if re.search('SECRET|TOKEN|PASSWORD|PASSWD|KEY|DATABASE_URL|DB_URL',key,re.I) and isinstance(value,str) and len(value)>3]
output=io.StringIO()
with contextlib.redirect_stdout(output):diagnostics=worker.collect_failure_diagnostics(attempt,secrets)
logs='\n'.join(line for line in output.getvalue().splitlines() if not line.startswith('CBGPU_DIAGNOSTICS '));names=', '.join(diagnostics['failed_services']) or 'startup/readiness check';error='Compose deployment failed: '+names+'. Containers and persistent data volumes were retained. See service status, health checks and container output in the deployment log.'
logs=((state['logs'] or '')[-25000:]+'\n'+logs)[-200000:];db.execute('UPDATE compose_states SET diagnostics=?,attempt_result=?,logs=?,error=?,updated_at=? WHERE project_id=?',(json.dumps(diagnostics),json.dumps(attempt),logs,error,datetime.datetime.now(datetime.timezone.utc).isoformat(),identifier));db.commit();db.close();print('Existing failed deployment diagnostics saved. Failed services: '+names+'. No containers or volumes were modified.')
