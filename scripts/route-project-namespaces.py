#!/usr/bin/env python3
import copy,datetime,importlib.util,json,os
from pathlib import Path
s=importlib.util.spec_from_file_location('r',Path(__file__).with_name('route-workspaces.py'));r=importlib.util.module_from_spec(s);s.loader.exec_module(r)
endpoint=f'accounts/{r.ACCOUNT}/cfd_tunnel/{r.TUNNEL}/configurations';before=r.api(endpoint);cfg=copy.deepcopy(before['config']);rules=cfg['ingress'];dns=f'zones/{r.ZONE}/dns_records';hostnames=['*.front.cbgpu.com','*.server.cbgpu.com'];snapshot={}
# Remove only the broad project wildcard introduced by this rollout.
for entry in rules:
 if entry.get('hostname')=='*.cbgpu.com' and entry.get('service')!='http://127.0.0.1:3510':raise RuntimeError('Broad wildcard belongs to another application')
rules[:]=[entry for entry in rules if entry.get('hostname')!='*.cbgpu.com']
for host in hostnames:
 existing=r.api(dns+'?name='+host.replace('*','%2A'));snapshot[host]=existing
 if any(x['type']!='CNAME' or x['content'].rstrip('.')!=r.TARGET for x in existing):raise RuntimeError('Conflicting DNS: '+host)
 matches=[x for x in rules if x.get('hostname')==host]
 if any(x.get('service')!='http://127.0.0.1:3510' for x in matches):raise RuntimeError('Conflicting route: '+host)
 if not matches:rules.insert(-1,{'hostname':host,'service':'http://127.0.0.1:3510'})
p=r.ROOT/'private'/('cloudflare-project-namespaces-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'.json');fd=os.open(p,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
with os.fdopen(fd,'w') as f:json.dump({'tunnel':before,'dns':snapshot},f)
if r.api(endpoint).get('version')!=before.get('version'):raise RuntimeError('Tunnel changed concurrently')
r.api(endpoint,'PUT',{'config':cfg})
for host,existing in snapshot.items():
 if not existing:r.api(dns,'POST',{'type':'CNAME','name':host,'content':r.TARGET,'proxied':True,'ttl':1})
for record in r.api(dns+'?name=%2A.cbgpu.com'):
 if record['type']=='CNAME' and record['content'].rstrip('.')==r.TARGET:r.api(dns+'/'+record['id'],'DELETE')
# Explicit validation TXT records accelerate initial namespace certificate issuance.
for pack in r.api(f'zones/{r.ZONE}/ssl/certificate_packs?status=all'):
 if pack.get('type')!='advanced' or not any(host in pack.get('hosts',[]) for host in ['front.cbgpu.com','server.cbgpu.com']):continue
 details=r.api(f'zones/{r.ZONE}/ssl/certificate_packs/'+pack['id'])
 for rec in details.get('validation_records',[]):
  if rec.get('status')!='pending' or rec.get('txt_name') not in ['_acme-challenge.front.cbgpu.com','_acme-challenge.server.cbgpu.com']:continue
  current=r.api(dns+'?type=TXT&name='+rec['txt_name'])
  if not any(x['content']==rec['txt_value'] for x in current):r.api(dns,'POST',{'type':'TXT','name':rec['txt_name'],'content':rec['txt_value'],'ttl':60})
print('Namespaced frontend/server routing published. Existing application hostnames preserved.')
