import json,urllib.request,urllib.error,subprocess,base64,time,hmac,hashlib,ssl,smtplib
from pathlib import Path
root=Path(__file__).resolve().parents[1]
env={k:v for line in (root/'.env').read_text().splitlines() if line and not line.startswith('#') and '=' in line for k,v in [line.split('=',1)]}
base='http://127.0.0.1:54321'
def api(path,token=None,body=None):
 headers={'apikey':env['SUPABASE_PUBLISHABLE_KEY'],'Authorization':'Bearer '+(token or env['ANON_KEY'])}
 if body is not None:headers['Content-Type']='application/json';body=json.dumps(body).encode()
 req=urllib.request.Request(base+path,data=body,headers=headers)
 try:
  with urllib.request.urlopen(req,timeout=30) as r:return r.status,json.loads(r.read() or b'null')
 except urllib.error.HTTPError as e:return e.code,json.loads(e.read() or b'null')
status,health=api('/auth/v1/health');assert status==200,(status,health);print('Auth health: OK')
status,data=api('/rest/v1/profiles?select=id');assert status==200 and data==[],(status,data);print('Anonymous access to profiles: denied by RLS')
status,data=api('/rest/v1/gpu_ssh_keys?select=*');assert status==200 and data==[],(status,data);print('Anonymous access to SSH keys: denied by RLS')
user=json.loads(subprocess.check_output(['docker','exec','cbgpu-supabase-db','psql','-U','postgres','-Atc',"select json_build_object('id',p.id,'email',u.email) from public.profiles p join auth.users u on u.id=p.id where coalesce(p.is_admin,false)=false and p.is_approved is distinct from false limit 1;"]).decode())
def b64(v):return base64.urlsafe_b64encode(v).decode().rstrip('=')
now=int(time.time());payload={'aud':'authenticated','role':'authenticated','sub':user['id'],'email':user['email'],'iss':env['API_EXTERNAL_URL'],'iat':now,'exp':now+300}
raw=b64(b'{"alg":"HS256","typ":"JWT"}')+'.'+b64(json.dumps(payload).encode());token=raw+'.'+b64(hmac.new(env['JWT_SECRET'].encode(),raw.encode(),hashlib.sha256).digest())
status,data=api('/auth/v1/user',token);assert status==200 and data['id']==user['id'],(status,data);print('Existing user token validated by Auth: OK')
status,data=api('/rest/v1/profiles?select=id&id=eq.'+user['id'],token);assert status==200 and len(data)==1 and data[0]['id']==user['id'];print('Authenticated own-profile lookup: OK')
status,data=api('/auth/v1/admin/generate_link',env['SERVICE_ROLE_KEY'],{'type':'recovery','email':user['email'],'redirect_to':'https://cbgpu.com/auth/reset-password'});assert status==200,(status,data)
link=data.get('action_link') or (data.get('properties') or {}).get('action_link');assert link and link.startswith('https://cbgpu.com/backend/auth/v1/verify?'),'Incorrect reset URL';print('Password reset link origin/path: OK (no email sent)')
status,data=api('/functions/v1/gpu-status');assert status==200,(status,data);print('GPU status Edge Function: OK')
status,data=api('/functions/v1/account-delete',body={});assert status in (401,403),(status,data);print('Unauthenticated account deletion: rejected')
print('Backend compatibility and security checks passed.')
