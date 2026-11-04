#!/usr/bin/env python3
"""Provision exact project TLS hostnames via a free Worker Custom Domain."""
import json,os,re,sys,urllib.request,urllib.error
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];PRIVATE=ROOT/'private';ACCOUNT='3a4fa468d49633b9a24c2c40539ea8a5';ZONE='99d6ebc0a8271e9b2104245b25c47529';WORKER='cbgpu-project-domains'
def api(route,method='GET',data=None,content_type='application/json'):
 token=json.loads((PRIVATE/'deploy-cloudflare-token.json').read_text())['token']
 body=None if data is None else data if isinstance(data,bytes) else json.dumps(data).encode()
 request=urllib.request.Request('https://api.cloudflare.com/client/v4/'+route,data=body,method=method,headers={'Authorization':'Bearer '+token,'Content-Type':content_type})
 try:
  with urllib.request.urlopen(request,timeout=30) as response:value=json.load(response)
 except urllib.error.HTTPError as e:
  try:value=json.load(e)
  except Exception:raise RuntimeError('Domain service returned HTTP '+str(e.code)) from None
 if not value.get('success'):raise RuntimeError('Domain service rejected the request: '+json.dumps(value.get('errors',[])))
 return value.get('result')
def ensure(host):
 if not re.fullmatch(r'[a-z][a-z0-9-]{2,60}\.(front|server)\.cbgpu\.com',host):raise ValueError('Invalid project domain')
 existing=api(f'accounts/{ACCOUNT}/workers/domains')
 match=next((v for v in existing if v.get('hostname')==host),None)
 if match:
  if match.get('service')!=WORKER:raise RuntimeError('Existing domain belongs to another application')
  return match
 return api(f'accounts/{ACCOUNT}/workers/domains','PUT',{'hostname':host,'service':WORKER,'environment':'production','zone_id':ZONE})
def remove(host):
 for value in api(f'accounts/{ACCOUNT}/workers/domains'):
  if value.get('hostname')==host and value.get('service')==WORKER:api(f'accounts/{ACCOUNT}/workers/domains/'+value['id'],'DELETE')
def setup():
 key=PRIVATE/'deployment-edge-key'
 if not key.exists():
  fd=os.open(key,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
  with os.fdopen(fd,'w') as f:f.write(os.urandom(32).hex())
 code=(ROOT/'deploy/edge-worker.js').read_bytes();metadata={'main_module':'worker.js','compatibility_date':'2026-10-29','bindings':[]};boundary='cbgpu-'+os.urandom(16).hex()
 parts=[]
 for name,mime,data in [('metadata','application/json',json.dumps(metadata).encode()),('worker.js','application/javascript+module',code)]:parts.append(('--'+boundary+'\r\nContent-Disposition: form-data; name="'+name+'"; filename="'+name+'"\r\nContent-Type: '+mime+'\r\n\r\n').encode()+data+b'\r\n')
 api(f'accounts/{ACCOUNT}/workers/scripts/{WORKER}','PUT',b''.join(parts)+('--'+boundary+'--\r\n').encode(),'multipart/form-data; boundary='+boundary)
 print('Project-domain TLS proxy configured. Application builds and runtimes stay on this computer.')
if __name__=='__main__':
 try:
  if sys.argv[1]=='setup':setup()
  elif sys.argv[1]=='ensure':ensure(sys.argv[2]);print('Hostname registered:',sys.argv[2])
  elif sys.argv[1]=='remove':remove(sys.argv[2]);print('Hostname removed:',sys.argv[2])
 except Exception as e:print(str(e),file=sys.stderr);sys.exit(1)
