#!/usr/bin/env python3
"""Route project subdomains without changing any existing application hostname."""
import importlib.util,copy,datetime,json,os,sys
from pathlib import Path
spec=importlib.util.spec_from_file_location('routes',Path(__file__).with_name('route-workspaces.py'));routes=importlib.util.module_from_spec(spec);spec.loader.exec_module(routes)
def main():
 endpoint=f'accounts/{routes.ACCOUNT}/cfd_tunnel/{routes.TUNNEL}/configurations';before=routes.api(endpoint);config=copy.deepcopy(before['config']);rules=config['ingress'];host='*.cbgpu.com';service='http://127.0.0.1:3510';dns=f'zones/{routes.ZONE}/dns_records';records=routes.api(dns+'?name=%2A.cbgpu.com')
 if any(r['type']!='CNAME' or r.get('content','').rstrip('.')!=routes.TARGET for r in records):raise RuntimeError('Existing wildcard DNS conflicts; left unchanged')
 if any(r.get('hostname')==host and r.get('service')!=service for r in rules):raise RuntimeError('Existing wildcard route conflicts; left unchanged')
 if not any(r.get('hostname')==host for r in rules):rules.insert(-1,{'hostname':host,'service':service})
 # An exact hostname rule must stay before the wildcard rule.
 wildcard=next(i for i,r in enumerate(rules) if r.get('hostname')==host)
 if any(r.get('hostname') and r.get('hostname')!=host for r in rules[wildcard+1:]):raise RuntimeError('Wildcard would shadow an existing application')
 print('Project wildcard route prepared; preserving',len(before['config']['ingress']),'existing routes.')
 if '--apply' not in sys.argv:return
 if routes.api(endpoint).get('version')!=before.get('version'):raise RuntimeError('Tunnel changed concurrently')
 file=routes.ROOT/'private'/('cloudflare-deployments-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'.json');fd=os.open(file,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'w') as f:json.dump({'tunnel':before,'dns':records},f)
 if config!=before['config']:routes.api(endpoint,'PUT',{'config':config})
 if not records:routes.api(dns,'POST',{'type':'CNAME','name':host,'content':routes.TARGET,'proxied':True,'ttl':1})
 elif not records[0].get('proxied'):routes.api(dns+'/'+records[0]['id'],'PATCH',{'proxied':True})
 print('Wildcard project HTTPS routing published; existing application routes preserved.')
if __name__=='__main__':
 try:main()
 except Exception as e:print(str(e),file=sys.stderr);sys.exit(1)
