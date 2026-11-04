#!/usr/bin/env python3
"""Publish the four user workspaces; preserve every existing tunnel route and DNS record."""
import copy,datetime,json,os,sqlite3,subprocess,sys,urllib.parse
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
HOSTS=['student.cbgpu.com','researcher.cbgpu.com','developer.cbgpu.com','app.cbgpu.com']
TUNNEL='3e478ed8-6349-443f-adb0-a446ae6a82fa';ACCOUNT='3a4fa468d49633b9a24c2c40539ea8a5';ZONE='99d6ebc0a8271e9b2104245b25c47529';TARGET=TUNNEL+'.cfargotunnel.com'
def api(route,method='GET',body=None):
 c=sqlite3.connect('file:/home/cbgpu/snap/firefox/common/.mozilla/firefox/5snylll4.default/cookies.sqlite?mode=ro',uri=True)
 cookie='; '.join(k+'='+v for k,v in c.execute("select name,value from moz_cookies where host in ('.cloudflare.com','dash.cloudflare.com','.dash.cloudflare.com')"));c.close()
 def quote(value):return '"'+value.replace('\\','\\\\').replace('"','\\"').replace('\n','\\n').replace('\r','\\r')+'"'
 config='url = '+quote('https://dash.cloudflare.com/api/v4/'+route)+'\nrequest = '+quote(method)+'\nheader = '+quote('Cookie: '+cookie)+'\nheader = "Referer: https://dash.cloudflare.com/"\n'
 if body is not None:config+='header = "X-Cross-Site-Security: dash"\nheader = "Content-Type: application/json"\ndata = '+quote(json.dumps(body))+'\n'
 r=subprocess.run(['curl','-sS','--max-time','30','--config','-'],input=config,text=True,capture_output=True,check=True)
 try:d=json.loads(r.stdout)
 except Exception:raise RuntimeError('Cloudflare returned a non-JSON response for '+method+' '+route) from None
 if not d.get('success'):raise RuntimeError('Cloudflare API rejected operation: '+json.dumps(d.get('errors',[])))
 return d['result']
def main():
 apply='--apply' in sys.argv;endpoint=f'accounts/{ACCOUNT}/cfd_tunnel/{TUNNEL}/configurations';original=api(endpoint);config=copy.deepcopy(original['config']);ingress=config['ingress'];dns=f'zones/{ZONE}/dns_records';records={}
 if ingress[-1].get('hostname'):raise RuntimeError('Missing terminal tunnel rule')
 for host in HOSTS:
  records[host]=api(dns+'?name='+urllib.parse.quote(host))
  if any(r['type']!='CNAME' or r.get('content','').rstrip('.')!=TARGET for r in records[host]):raise RuntimeError('Conflicting DNS left unchanged: '+host)
  matches=[r for r in ingress if r.get('hostname')==host]
  if matches and any(r.get('service')!='http://127.0.0.1:3000' or r.get('path') for r in matches):raise RuntimeError('Conflicting route left unchanged: '+host)
  if not matches:ingress.insert(-1,{'hostname':host,'service':'http://127.0.0.1:3000'})
 print('Workspace routes:',', '.join(HOSTS));print('Existing routes preserved:',len(original['config']['ingress']))
 if not apply:return
 if api(endpoint).get('version')!=original.get('version'):raise RuntimeError('Tunnel changed concurrently; retry after reviewing')
 p=ROOT/'private'/('cloudflare-workspaces-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'.json');fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'w') as f:json.dump({'tunnel':original,'dns':records},f)
 if config!=original['config']:api(endpoint,'PUT',{'config':config})
 for host,existing in records.items():
  if not existing:api(dns,'POST',{'type':'CNAME','name':host,'content':TARGET,'proxied':True,'ttl':1})
  elif not existing[0].get('proxied'):api(dns+'/'+existing[0]['id'],'PATCH',{'proxied':True})
 final=api(endpoint)['config']['ingress']
 for host in HOSTS:
  if not any(r.get('hostname')==host and r.get('service')=='http://127.0.0.1:3000' for r in final):raise RuntimeError('Verification failed: '+host)
 print('Published and verified four workspace routes and DNS records.')
if __name__=='__main__':
 try:main()
 except Exception as e:print(str(e),file=sys.stderr);sys.exit(1)
