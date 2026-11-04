from pathlib import Path
import subprocess,shlex,json,urllib.request,urllib.error,datetime
root=Path(__file__).resolve().parents[1]
expected={
 'resend._domainkey.cbgpu.com':('TXT','p=MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCo8NfPHtaXx7WxDyCt/cKPRDCKHxC1PtsHbFrgH+BTeQqdARVXiOAtUEsm8fw9Wk4waq9GN5j9gLiE2A8FMWhgVbzLqhCeu6Sf/XOcsWX1UpMacoKmAFQks3Gti4orfnbWkFDma646w7ftyERpN+oBgEx5gTwY7O2QljPMcQ0B7QIDAQAB'),
 'rsend.cbgpu.com':('CNAME','rsend-apne1.forge.rmta.net'),
 'send.cbgpu.com':('CNAME','send.forge.rmta.net'),
 '_dmarc.cbgpu.com':('TXT','v=DMARC1; p=none;'),
}
records=[]
for name,(kind,value) in expected.items():
 result=subprocess.run(['dig','+short',kind,name],capture_output=True,text=True,timeout=10)
 observed=[''.join(shlex.split(l)) if kind=='TXT' else l.rstrip('.') for l in result.stdout.splitlines() if l]
 records.append({'name':name,'type':kind,'matches':value in observed,'observed':observed,'expected':value})
status={'checked_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'dns_ready':all(r['matches'] for r in records),'records':records,'resend_status':'not_accessible_with_sending_only_key'}
keyfile=root/'private/resend-management-key'
if keyfile.exists():
 try:
  req=urllib.request.Request('https://api.resend.com/domains',headers={'Authorization':'Bearer '+keyfile.read_text().strip(),'User-Agent':'CBGPU domain monitor'})
  with urllib.request.urlopen(req,timeout=15) as response:data=json.load(response)
  domain=next((d for d in data.get('data',[]) if d['name']=='cbgpu.com'),None)
  status['resend_status']=domain['status'] if domain else 'not_in_this_account'
 except urllib.error.HTTPError as e:status['resend_status']='unavailable_http_'+str(e.code)
path=root/'private/email-domain-status.json';path.write_text(json.dumps(status,ensure_ascii=False,indent=2));path.chmod(0o600)
print(json.dumps({k:status[k] for k in ['checked_at','dns_ready','resend_status']}))
