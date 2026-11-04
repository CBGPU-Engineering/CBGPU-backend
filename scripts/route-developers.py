#!/usr/bin/env python3
"""Add only developers.cbgpu.com to the existing Cloudflare tunnel and DNS."""
import argparse,copy,json,os,stat,sys,urllib.request,urllib.error,urllib.parse,datetime
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
TOKEN=Path('/home/cbgpu/.config/cbgpu/cloudflare-routing-token')
TUNNEL='3e478ed8-6349-443f-adb0-a446ae6a82fa'
HOST='developers.cbgpu.com'
SERVICE='http://127.0.0.1:3400'
TARGET=TUNNEL+'.cfargotunnel.com'

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--apply',action='store_true');args=parser.parse_args()
 if not TOKEN.exists():raise RuntimeError('Save the scoped Cloudflare API token in '+str(TOKEN)+' with permissions 600 first.')
 if stat.S_IMODE(TOKEN.stat().st_mode)&0o077:raise RuntimeError('Token file must have permissions 600.')
 token=TOKEN.read_text().strip()
 def api(route,method='GET',body=None):
  req=urllib.request.Request('https://api.cloudflare.com/client/v4/'+route,method=method,headers={'Authorization':'Bearer '+token,'Content-Type':'application/json','User-Agent':'CBGPU routing setup'},data=json.dumps(body).encode() if body is not None else None)
  try:
   with urllib.request.urlopen(req,timeout=30) as response:data=json.load(response)
  except urllib.error.HTTPError as e:
   try:detail=json.load(e).get('errors',[])
   except Exception:detail=[]
   raise RuntimeError('Cloudflare HTTP '+str(e.code)+': '+json.dumps(detail).replace(token,'[redacted]')) from None
  if not data.get('success'):raise RuntimeError('Cloudflare API error: '+json.dumps(data.get('errors',[])).replace(token,'[redacted]'))
  return data.get('result')
 zones=api('zones?name=cbgpu.com');zones=[z for z in zones if z['name']=='cbgpu.com']
 if len(zones)!=1:raise RuntimeError('The token must have access to the cbgpu.com DNS zone.')
 zone=zones[0];endpoint='accounts/'+zone['account']['id']+'/cfd_tunnel/'+TUNNEL+'/configurations'
 original=api(endpoint);config=copy.deepcopy(original['config']);ingress=config.get('ingress',[])
 if not ingress or ingress[-1].get('hostname'):raise RuntimeError('Tunnel has no final catch-all rule; inspect it before changing routes.')
 dns_endpoint='zones/'+zone['id']+'/dns_records';records=api(dns_endpoint+'?name='+urllib.parse.quote(HOST))
 if any(r['type']!='CNAME' or r.get('content','').rstrip('.')!=TARGET for r in records):raise RuntimeError('A conflicting DNS record exists for '+HOST+'. It was left unchanged.')
 matches=[i for i,r in enumerate(ingress) if r.get('hostname')==HOST and not r.get('path')]
 if len(matches)>1:raise RuntimeError('Duplicate hostname routes found; inspect them before continuing.')
 desired={'hostname':HOST,'service':SERVICE}
 if matches:ingress[matches[0]]=desired
 else:ingress.insert(0,desired)
 print(json.dumps({'hostname':HOST,'service':SERVICE,'dns_target':TARGET,'preserved_routes':[r.get('hostname','catch-all') for r in original['config']['ingress'] if r.get('hostname')!=HOST],'mode':'apply' if args.apply else 'check'},indent=2))
 if not args.apply:return
 # Re-check before writing so a concurrent dashboard edit is not silently lost.
 if api(endpoint).get('version')!=original.get('version'):raise RuntimeError('Tunnel changed during this operation. Retry after reviewing it.')
 backup=ROOT/'private'/('cloudflare-routes-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'.json')
 descriptor=os.open(backup,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(descriptor,'w') as f:json.dump(original,f)
 changed=config!=original['config']
 if changed:api(endpoint,'PUT',{'config':config})
 try:
  if not records:api(dns_endpoint,'POST',{'type':'CNAME','name':HOST,'content':TARGET,'proxied':True,'ttl':1})
  elif not records[0].get('proxied'):api(dns_endpoint+'/'+records[0]['id'],'PATCH',{'proxied':True})
 except Exception:
  # Restore only our own config if no other edit has occurred since the write.
  if changed and api(endpoint).get('config')==config:api(endpoint,'PUT',{'config':original['config']})
  raise
 final=api(endpoint)
 if not any(r.get('hostname')==HOST and r.get('service')==SERVICE for r in final['config']['ingress']):raise RuntimeError('Route verification failed.')
 print('Cloudflare route and DNS configured. Next verify https://'+HOST+' and its authenticated terminal.')

if __name__=='__main__':
 try:main()
 except Exception as e:print(str(e),file=sys.stderr);sys.exit(1)
