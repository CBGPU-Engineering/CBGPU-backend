#!/usr/bin/env python3
"""External-domain TLS adapter. Never enables billing or exceeds the included quota."""
import importlib.util,json,sys
from pathlib import Path
p=Path(__file__).with_name('cloudflare_domains.py');s=importlib.util.spec_from_file_location('domains',p);d=importlib.util.module_from_spec(s);s.loader.exec_module(d)
try:
 if sys.argv[1]=='remove':d.api(f'zones/{d.ZONE}/custom_hostnames/'+sys.argv[2],'DELETE');print('{}');sys.exit()
 host=sys.argv[2];identifier=sys.argv[3] if len(sys.argv)>3 else ''
 if identifier:value=d.api(f'zones/{d.ZONE}/custom_hostnames/'+identifier)
 else:
  current=d.api(f'zones/{d.ZONE}/custom_hostnames?per_page=100')
  value=next((r for r in current if r.get('hostname')==host),None)
  if value is None:
   if len(current)>=90:raise RuntimeError('Included hostname capacity reached')
   fallback=d.api(f'zones/{d.ZONE}/custom_hostnames/fallback_origin')
   if fallback.get('status')!='active':raise RuntimeError('Custom TLS fallback origin needs administrator configuration')
   value=d.api(f'zones/{d.ZONE}/custom_hostnames','POST',{'hostname':host,'ssl':{'method':'http','type':'dv','settings':{'min_tls_version':'1.2'}}})
 records=[];ownership=value.get('ownership_verification') or {}
 if ownership.get('name') and ownership.get('value'):records.append({'type':ownership.get('type','TXT').upper(),'name':ownership['name'],'value':ownership['value']})
 for rec in value.get('ssl',{}).get('validation_records') or []:
  if rec.get('txt_name') and rec.get('txt_value'):records.append({'type':'TXT','name':rec['txt_name'],'value':rec['txt_value']})
 active=value.get('status')=='active' and value.get('ssl',{}).get('status')=='active'
 print(json.dumps({'id':value['id'],'status':'active' if active else 'pending_tls','records':records}))
except Exception:
 print(json.dumps({'status':'provider_setup_required','records':[]}))
