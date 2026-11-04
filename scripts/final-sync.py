"""Refresh the prepared local backend after the source write guard is enabled."""
from pathlib import Path
import json,subprocess,re
root=Path(__file__).resolve().parents[1]
private=root/'private';state=private/'cutover'
if not (state/'controller-paused').exists():raise SystemExit('The root GPU controller must be paused with host-cutover.sh first.')
source=lambda sql: subprocess.check_output([str(private/'read-source.sh'),'-Atc',sql],cwd=root,text=True)
if int(source("select count(*) from pg_trigger where tgname='cbgpu_backend_moved' and not tgisinternal;"))<37:
 raise SystemExit('Source write guard is not active; refusing an inconsistent cutover.')
subprocess.run([str(root/'export-source.sh')],cwd=root,check=True)
listing=subprocess.check_output(['docker','run','--rm','--entrypoint','pg_restore','-v',str(private)+':/private:ro','supabase/postgres:17.6.1.136','--list','/private/source-full.dump'],text=True)
entries=[];tables=[]
for line in listing.splitlines():
 m=re.search(r' TABLE DATA (public|auth) (\S+) ',line)
 if m and not (m.group(1)=='auth' and m.group(2)=='schema_migrations'):
  entries.append(line);tables.append((m.group(1),m.group(2)))
 if re.search(r' SEQUENCE SET (public|auth) ',line):entries.append(line)
p=private/'final-data.list';p.write_text('\n'.join(entries)+'\n')
subprocess.run(['docker','cp',str(private/'source-full.dump'),'cbgpu-supabase-db:/tmp/cbgpu-final.dump'],check=True)
subprocess.run(['docker','cp',str(p),'cbgpu-supabase-db:/tmp/cbgpu-final.list'],check=True)
subprocess.run(['docker','exec','cbgpu-supabase-db','pg_restore','--data-only','--use-list=/tmp/cbgpu-final.list','--file=/tmp/cbgpu-final-data.sql','/tmp/cbgpu-final.dump'],check=True)
quote=lambda s:'"'+s.replace('"','""')+'"'
truncate='TRUNCATE '+','.join(quote(s)+'.'+quote(t) for s,t in tables)+' RESTART IDENTITY CASCADE;'
command=['docker','exec','cbgpu-supabase-db','psql','-U','supabase_admin','-v','ON_ERROR_STOP=1','--single-transaction','-c','SET session_replication_role = replica','-c',truncate,'-f','/tmp/cbgpu-final-data.sql']
with (private/'final-data-restore.log').open('w') as output:subprocess.run(command,stdout=output,stderr=output,check=True)
# Fetch the final file list and ownership metadata without printing user filenames.
(private/'storage-objects.json').write_text(source("select coalesce(json_agg(json_build_object('bucket',bucket_id,'name',name,'metadata',metadata)),'[]') from storage.objects;"))
(private/'storage-ownership.json').write_text(source("select coalesce(json_agg(json_build_object('id',id,'bucket_id',bucket_id,'name',name,'owner',owner,'owner_id',owner_id,'created_at',created_at,'updated_at',updated_at,'user_metadata',user_metadata)),'[]') from storage.objects;"))
subprocess.run(['python3',str(private/'transfer-storage.py')],cwd=root,check=True)
hex_data=(private/'storage-ownership.json').read_bytes().hex()
fix="""with source as (select * from jsonb_to_recordset(convert_from(decode('"""+hex_data+"""','hex'),'UTF8')::jsonb) as r(id uuid,bucket_id text,name text,owner uuid,owner_id text,created_at timestamptz,updated_at timestamptz,user_metadata jsonb)) update storage.objects o set id=s.id,owner=s.owner,owner_id=s.owner_id,created_at=s.created_at,updated_at=s.updated_at,user_metadata=s.user_metadata from source s where o.bucket_id=s.bucket_id and o.name=s.name;
DO $$ DECLARE r record; BEGIN FOR r IN SELECT table_name,column_name FROM information_schema.columns WHERE table_schema='public' AND data_type IN ('text','character varying') LOOP EXECUTE format('UPDATE public.%I SET %I=replace(%I,$1,$2) WHERE position($1 in %I)>0',r.table_name,r.column_name,r.column_name,r.column_name) USING 'https://fclbzjlcepoaifvcoxdw.supabase.co','https://cbgpu.com/backend'; END LOOP; END $$;
"""
r=subprocess.run(['docker','exec','-i','cbgpu-supabase-db','psql','-U','supabase_admin','-v','ON_ERROR_STOP=1','--single-transaction','-c','SET session_replication_role = replica','-f','-'],input=fix,text=True,capture_output=True)
if r.returncode:raise SystemExit('Final URL/ownership updates failed; details withheld to protect private records.')
print('Final application data, account records, file bytes, and file ownership synchronized.')
