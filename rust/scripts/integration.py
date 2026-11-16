#!/usr/bin/env python3
"""Real PostgreSQL tests in a throwaway database, never in application databases."""
import os,subprocess
from pathlib import Path
from urllib.parse import urlsplit,urlunsplit
ROOT=Path(__file__).resolve().parents[1]
def sql(query,database='postgres'):
 r=subprocess.run(['docker','exec','-i','cbgpu-supabase-db','psql','-X','-q','-v','ON_ERROR_STOP=1','-U','postgres','-d',database],input=query,text=True,capture_output=True,timeout=20)
 if r.returncode:raise RuntimeError('Isolated database fixture operation failed.')
values=dict(line.split('=',1) for line in (Path.home()/'.config/cbgpu/rust-core.env').read_text().splitlines() if '=' in line)
url=urlsplit(values['CBGPU_RUNTIME_DATABASE_URL']);test_url=urlunsplit(url._replace(path='/cbgpu_runtime_test'))
# No IF EXISTS/CASCADE: refuse collisions with an existing test database.
sql('GRANT cbgpu_runtime_owner TO postgres WITH SET TRUE;')
try:
 sql('CREATE DATABASE cbgpu_runtime_test OWNER cbgpu_runtime_owner;')
 try:
  migration=next((ROOT/'migrations').glob('*_rust_operations_core.sql')).read_text()
  sql('REVOKE ALL ON DATABASE cbgpu_runtime_test FROM PUBLIC; GRANT CONNECT ON DATABASE cbgpu_runtime_test TO cbgpu_runtime; SET ROLE cbgpu_runtime_owner;'+migration,'cbgpu_runtime_test')
  result=subprocess.run([str(Path.home()/'.cargo/bin/cargo'),'test','--locked','--manifest-path',str(ROOT/'Cargo.toml'),'--test','postgres','--','--ignored'],env={**os.environ,'CBGPU_TEST_DATABASE_URL':test_url,'CARGO_BUILD_JOBS':'4'})
 finally:sql('DROP DATABASE cbgpu_runtime_test;')
finally:sql('REVOKE cbgpu_runtime_owner FROM postgres;')
raise SystemExit(result.returncode)
