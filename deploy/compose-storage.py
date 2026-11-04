#!/usr/bin/env python3
import json,re,subprocess,sys
cfg=json.load(sys.stdin);namespace=cfg['namespace']
if not re.fullmatch(r'cbgpu-compose-[a-f0-9-]{36}',namespace):raise ValueError('Invalid namespace')
volumes=[v for v in cfg.get('volumes',[]) if isinstance(v,str) and v.startswith(namespace+'-')]
args=['docker','run','--rm','--network','none','--read-only','--memory','64m','--cpus','0.25','--pids-limit','32','--entrypoint','/bin/sh']
for i,name in enumerate(volumes):args+=['--mount','type=volume,src='+name+',dst=/v'+str(i)+',readonly']
size=0
if volumes:
 result=subprocess.run(args+['nginx:alpine','-c','du -sk '+' '.join('/v'+str(i) for i in range(len(volumes)))],capture_output=True,text=True,timeout=90,check=True)
 size+=sum(int(line.split()[0])*1024 for line in result.stdout.splitlines() if line.split())
ids=subprocess.run(['docker','ps','-a','--filter','label=com.docker.compose.project='+namespace,'--format','{{.ID}}'],capture_output=True,text=True,timeout=10,check=True).stdout.split()
if ids:
 result=subprocess.run(['docker','inspect','--size','--format','{{.SizeRw}}',*ids],capture_output=True,text=True,timeout=90,check=True)
 size+=sum(int(line) for line in result.stdout.splitlines() if line.isdigit())
print(json.dumps({'bytes':size}))
