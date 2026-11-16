#!/usr/bin/env python3
"""Provision an isolated runtime database; never change existing application tables."""
import json, os, secrets, subprocess
from pathlib import Path
from urllib.parse import quote
ROOT=Path(__file__).resolve().parents[1]
CONFIG=Path.home()/'.config/cbgpu'
CONTAINER='cbgpu-supabase-db'
def sql(query,database='postgres'):
 result=subprocess.run(['docker','exec','-i',CONTAINER,'psql','-X','-A','-t','-q','-v','ON_ERROR_STOP=1','-U','postgres','-d',database],input=query,text=True,capture_output=True,timeout=20)
 if result.returncode:raise RuntimeError('Runtime database provisioning failed; no credentials were logged.')
 return result.stdout.strip()
def main():
 os.umask(0o077);CONFIG.mkdir(mode=0o700,parents=True,exist_ok=True)
 env_file=CONFIG/'rust-core.env'
 if env_file.exists():raise SystemExit('Runtime configuration already exists. Verify it instead of replacing credentials.')
 role_exists=sql("SELECT count(*) FROM pg_roles WHERE rolname IN ('cbgpu_runtime','cbgpu_runtime_owner');")
 db_exists=sql("SELECT count(*) FROM pg_database WHERE datname='cbgpu_runtime';")
 if role_exists!='0' or db_exists!='0':raise SystemExit('Runtime role or database already exists; refusing to overwrite it.')
 password=secrets.token_hex(32);owner_password=secrets.token_hex(32)
 sql(f"CREATE ROLE cbgpu_runtime LOGIN PASSWORD '{password}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT CONNECTION LIMIT 8; CREATE ROLE cbgpu_runtime_owner LOGIN PASSWORD '{owner_password}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT CONNECTION LIMIT 2;")
 sql("GRANT cbgpu_runtime_owner TO postgres WITH SET TRUE;")
 sql("CREATE DATABASE cbgpu_runtime OWNER cbgpu_runtime_owner;")
 sql("REVOKE cbgpu_runtime_owner FROM postgres;")
 sql("REVOKE ALL ON DATABASE cbgpu_runtime FROM PUBLIC; GRANT CONNECT ON DATABASE cbgpu_runtime TO cbgpu_runtime;",'cbgpu_runtime')
 sql("REVOKE CREATE ON SCHEMA public FROM PUBLIC;",'cbgpu_runtime')
 ip=subprocess.check_output(['docker','inspect','--format','{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}',CONTAINER],text=True).strip()
 prefix=f'postgresql://cbgpu_runtime:'
 runtime_url=prefix+quote(password,safe='')+'@'+ip+':5432/cbgpu_runtime'
 owner_url='postgresql://cbgpu_runtime_owner:'+quote(owner_password,safe='')+'@'+ip+':5432/cbgpu_runtime'
 binary=ROOT/'target/release/cbgpu-core'
 result=subprocess.run([str(binary),'migrate'],env={**os.environ,'CBGPU_RUNTIME_DATABASE_URL':owner_url},capture_output=True,text=True,timeout=60)
 if result.returncode:raise RuntimeError('Runtime schema migration failed. Runtime roles retained for inspection; credentials were not logged.')
 sql('ALTER ROLE cbgpu_runtime_owner NOLOGIN;')
 key_file=CONFIG/'rust-core-admin-key'
 if not key_file.exists():key_file.write_text(secrets.token_hex(32)+'\n');key_file.chmod(0o600)
 env_file.write_text('CBGPU_RUNTIME_DATABASE_URL='+runtime_url+'\nCBGPU_RUST_LISTEN=127.0.0.1:3540\nCBGPU_WEB_ORIGIN=http://127.0.0.1:3000\nCBGPU_RUST_ADMIN_KEY_FILE='+str(key_file)+'\n')
 env_file.chmod(0o600)
 print(json.dumps({'ok':True,'database':'cbgpu_runtime','runtime_superuser':False,'runtime_owns_tables':False,'existing_application_tables_changed':False}))
if __name__=='__main__':main()
