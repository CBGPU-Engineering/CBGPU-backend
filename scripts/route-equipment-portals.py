#!/usr/bin/env python3
"""Publish independent printer/laser portals and replace the former combined route with a chooser."""
import copy,datetime,importlib.util,json,os,sys,urllib.parse
from pathlib import Path
source=Path(__file__).with_name('route-workspaces.py');spec=importlib.util.spec_from_file_location('workspace_routes',source);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
def main():
 hosts=['print.cbgpu.com','laser.cbgpu.com','manufacturing.cbgpu.com'];service='http://127.0.0.1:3400';endpoint=f'accounts/{m.ACCOUNT}/cfd_tunnel/{m.TUNNEL}/configurations';original=m.api(endpoint);config=copy.deepcopy(original['config']);ingress=config['ingress'];dns=f'zones/{m.ZONE}/dns_records';records={}
 if ingress[-1].get('hostname'):raise RuntimeError('Terminal tunnel rule missing')
 for host in hosts:
  records[host]=m.api(dns+'?name='+urllib.parse.quote(host))
  if any(r['type']!='CNAME' or r.get('content','').rstrip('.')!=m.TARGET for r in records[host]):raise RuntimeError('DNS conflict left unchanged: '+host)
  matches=[r for r in ingress if r.get('hostname')==host]
  allowed={service,'http://127.0.0.1:3000'} if host=='manufacturing.cbgpu.com' else {service}
  if matches and any(r.get('service') not in allowed or r.get('path') for r in matches):raise RuntimeError('Route conflict left unchanged: '+host)
  if matches:
   for route in matches:route['service']=service
  else:ingress.insert(-1,{'hostname':host,'service':service})
 print('Equipment gateway routes ready:',', '.join(hosts));print('Existing routes preserved:',len(original['config']['ingress']))
 if '--apply' not in sys.argv:return
 if m.api(endpoint).get('version')!=original.get('version'):raise RuntimeError('Tunnel changed concurrently; review again')
 backup=m.ROOT/'private'/('cloudflare-equipment-portals-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'.json')
 with os.fdopen(os.open(backup,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600),'w') as f:json.dump({'tunnel':original,'dns':records},f)
 if config!=original['config']:m.api(endpoint,'PUT',{'config':config})
 for host in hosts:
  if not records[host]:m.api(dns,'POST',{'type':'CNAME','name':host,'content':m.TARGET,'proxied':True,'ttl':1})
  elif not records[host][0].get('proxied'):m.api(dns+'/'+records[host][0]['id'],'PATCH',{'proxied':True})
 final=m.api(endpoint)['config']['ingress']
 for host in hosts:assert any(r.get('hostname')==host and r.get('service')==service for r in final)
 print('Published separate equipment portals and the legacy chooser; unrelated routes unchanged.')
if __name__=='__main__':main()
