import fs from 'node:fs';import path from 'node:path';import os from 'node:os';
export function studyHealth(file=path.join(os.homedir(),'.local/share/cbgpu-study-monitor/status.json'),now=Date.now()){
 try{
  if(fs.statSync(file).size>16000)throw Error('oversize');const value=JSON.parse(fs.readFileSync(file,'utf8')),checked=Date.parse(value.checked_at);
  if(!Number.isFinite(checked)||checked>now+60000||now-checked>15*60000)return {status:'unavailable',detail:'Study lookup check is overdue.'};
  let status=['healthy','degraded','unavailable'].includes(value.status)?value.status:'unavailable';
  if(!Number.isInteger(value.rooms)||value.rooms<1||!Number.isInteger(value.failed_rooms)||value.failed_rooms<0||value.failed_rooms>value.rooms)status='unavailable';
  else if(status==='healthy'&&value.failed_rooms>0)status='degraded';
  return {status,detail:status==='healthy'?'School records and layouts verified.':value.detail==='shared_connection_expired'?'Shared school connection expired; renew it in admin.':value.detail==='shared_connection_expiring'?'Shared school connection expires within 24 hours; renew it in admin.':status==='degraded'?'Some room layouts require attention.':'School connection or response validation failed.',checked_at:value.checked_at};
 }catch{return {status:'unavailable',detail:'Study lookup monitor is not configured or unreadable.'}}
}
