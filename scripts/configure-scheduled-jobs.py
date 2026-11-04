from pathlib import Path
import json,subprocess
root=Path(__file__).resolve().parents[1]
env={k:v for l in (root/'.env').read_text().splitlines() if '=' in l and not l.startswith('#') for k,v in [l.split('=',1)]}
rows=json.loads((root/'private/source-cron.json').read_text())
quote=lambda s:"convert_from(decode('"+s.encode().hex()+"','hex'),'UTF8')"
parts=["select vault.create_secret("+quote(env['SERVICE_ROLE_KEY'])+",'cbgpu_service_role_key','Local CBGPU backend service key') where not exists(select 1 from vault.secrets where name='cbgpu_service_role_key');"]
for job in rows:
 command=job['command'].replace('https://fclbzjlcepoaifvcoxdw.supabase.co','http://api-gw:8000')
 parts.append('select cron.schedule('+quote(job['jobname'])+','+quote(job['schedule'])+','+quote(command)+');')
 parts.append('select cron.alter_job(jobid,active:=false) from cron.job where jobname='+quote(job['jobname'])+';')
r=subprocess.run(['docker','exec','-i','cbgpu-supabase-db','psql','-U','postgres','-v','ON_ERROR_STOP=1','--single-transaction'],input='\n'.join(parts),capture_output=True,text=True)
if r.returncode: print('Scheduled job configuration failed');raise SystemExit(1)
print('Scheduled tasks copied, repointed to local functions, and kept inactive until cutover.')
