#!/usr/bin/env python3
"""Publish only the authenticated LaTeX gateway; preserve all other routes and DNS."""
import copy,datetime,importlib.util,json,os,sys,urllib.parse
from pathlib import Path
source=Path(__file__).with_name('route-workspaces.py');spec=importlib.util.spec_from_file_location('workspace_routes',source);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
def main():
 host='latex.cbgpu.com';service='http://127.0.0.1:3520';endpoint=f'accounts/{m.ACCOUNT}/cfd_tunnel/{m.TUNNEL}/configurations';original=m.api(endpoint);config=copy.deepcopy(original['config']);ingress=config['ingress'];dns=f'zones/{m.ZONE}/dns_records';records=m.api(dns+'?name='+urllib.parse.quote(host))
 if any(r['type']!='CNAME' or r.get('content','').rstrip('.')!=m.TARGET for r in records):raise RuntimeError('Existing LaTeX DNS conflicts; left unchanged')
 matches=[r for r in ingress if r.get('hostname')==host]
 if matches and any(r.get('service')!=service or r.get('path') for r in matches):raise RuntimeError('Existing LaTeX route conflicts; left unchanged')
 if ingress[-1].get('hostname'):raise RuntimeError('Terminal tunnel rule missing')
 if not matches:ingress.insert(-1,{'hostname':host,'service':service})
 print('LaTeX gateway route ready; existing routes preserved:',len(original['config']['ingress']))
 if '--apply' not in sys.argv:return
 if m.api(endpoint).get('version')!=original.get('version'):raise RuntimeError('Tunnel changed concurrently; review again')
 backup=m.ROOT/'private'/('cloudflare-latex-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'.json')
 with os.fdopen(os.open(backup,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600),'w') as f:json.dump({'tunnel':original,'dns':records},f)
 if config!=original['config']:m.api(endpoint,'PUT',{'config':config})
 if not records:m.api(dns,'POST',{'type':'CNAME','name':host,'content':m.TARGET,'proxied':True,'ttl':1})
 elif not records[0].get('proxied'):m.api(dns+'/'+records[0]['id'],'PATCH',{'proxied':True})
 assert any(r.get('hostname')==host and r.get('service')==service for r in m.api(endpoint)['config']['ingress'])
 print('Published and verified latex.cbgpu.com; no other routes were changed.')
if __name__=='__main__':main()
