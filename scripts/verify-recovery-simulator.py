#!/usr/bin/env python3
"""Exercise real Auth recovery SMTP against Resend's non-human delivery simulator."""
import json,subprocess,uuid,secrets,time,urllib.request,urllib.parse,urllib.error
from pathlib import Path
root=Path(__file__).resolve().parents[1];env={}
for line in (root/'.env').read_text().splitlines():
 if '='in line and not line.startswith('#'):
  key,value=line.split('=',1);env[key]=value.strip().strip('\"\'')
def sql(query):return subprocess.check_output(['docker','exec','cbgpu-supabase-db','psql','-U','supabase_admin','-d','postgres','-At','-v','ON_ERROR_STOP=1','-c',query],text=True).strip()
address='delivered@resend.dev';identifier=str(uuid.uuid4());password=secrets.token_urlsafe(24);created=False
if sql("SELECT count(*) FROM auth.users WHERE email='"+address+"'")!='0':raise RuntimeError('Simulator account already exists; refusing to modify it')
try:
 for _ in range(30):
  try:
   with urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:54321/auth/v1/health',headers={'apikey':env.get('SUPABASE_PUBLISHABLE_KEY') or env['ANON_KEY']}),timeout=2) as response:
    if response.status==200:break
  except Exception:time.sleep(1)
 sql(f"BEGIN;SET LOCAL session_replication_role=replica;INSERT INTO auth.users(instance_id,id,aud,role,email,encrypted_password,email_confirmed_at,confirmation_token,recovery_token,email_change_token_new,email_change,raw_app_meta_data,raw_user_meta_data,created_at,updated_at) VALUES('00000000-0000-0000-0000-000000000000','{identifier}','authenticated','authenticated','{address}',extensions.crypt('{password}',extensions.gen_salt('bf')),now(),'','','','','{{\"provider\":\"email\",\"providers\":[\"email\"]}}','{{}}',now(),now());INSERT INTO auth.identities(provider_id,user_id,identity_data,provider,created_at,updated_at) VALUES('{identifier}','{identifier}','{{\"sub\":\"{identifier}\",\"email\":\"{address}\",\"email_verified\":true}}','email',now(),now());COMMIT;");created=True
 request=urllib.request.Request('http://127.0.0.1:54321/auth/v1/recover?'+urllib.parse.urlencode({'redirect_to':'https://cbgpu.com/auth/reset-password'}),data=json.dumps({'email':address}).encode(),headers={'apikey':env.get('SUPABASE_PUBLISHABLE_KEY') or env['ANON_KEY'],'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(request,timeout=30) as response:print('Native recovery endpoint HTTP:',response.status)
 except urllib.error.HTTPError as error:
  payload=json.load(error);print('Recovery failed:',error.code,payload.get('msg') or payload.get('message'));raise SystemExit(1)
 result=sql(f"SELECT recovery_sent_at IS NOT NULL AND length(recovery_token)>0 FROM auth.users WHERE id='{identifier}'")
 if result!='t':raise RuntimeError('Recovery state was not committed')
 print('Password recovery SMTP and branded template path verified using the Resend test simulator. No real user received a test email.')
finally:
 if created:sql(f"DELETE FROM auth.users WHERE id='{identifier}'");print('Temporary simulator account removed.')
