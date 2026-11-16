#!/usr/bin/env node
// Operator-only read probe: no user sessions, application writes or student data logs.
import fs from 'node:fs/promises';import path from 'node:path';import os from 'node:os';import {createRequire} from 'node:module';import {parseEnv} from 'node:util';import {pathToFileURL} from 'node:url';import crypto from 'node:crypto';
const folder=path.join(os.homedir(),'.local/share/cbgpu-study-monitor'),statusFile=path.join(folder,'status.json');
let status: {status:string;checked_at:string;detail:string;rooms?:number;failed_rooms?:number;connection_expires_at?:string}={status:'unavailable',checked_at:new Date().toISOString(),detail:'Study read check failed.'};
try{
 const unit=await fs.readFile(path.join(os.homedir(),'.config/systemd/user/cbgpu-frontend.service'),'utf8');const root=unit.match(/^WorkingDirectory=(.+)$/m)?.[1];if(!root)throw Error('active_release');
 const env=parseEnv(await fs.readFile(path.join(root,'.env.production.local'),'utf8')),require=createRequire(path.join(root,'package.json'));
 const {createClient}=require('@supabase/supabase-js');const db=createClient('http://127.0.0.1:54321',env.NEXT_PUBLIC_SUPABASE_ANON_KEY,{auth:{persistSession:false,autoRefreshToken:false}});
 const {data,error}=await db.from('eduset_settings').select('access_token_enc').eq('id',1).maybeSingle();if(error||!data?.access_token_enc)throw Error('shared_connection_missing');
 const bytes=Buffer.from(data.access_token_enc,'base64'),dec=crypto.createDecipheriv('aes-256-gcm',crypto.createHash('sha256').update(env.EDUSET_TOKEN_SECRET).digest(),bytes.subarray(0,12));dec.setAuthTag(bytes.subarray(12,28));const token=Buffer.concat([dec.update(bytes.subarray(28)),dec.final()]).toString();
 const claims=JSON.parse(Buffer.from(token.split('.')[1],'base64url').toString('utf8'));if(!Number.isFinite(claims.exp)||claims.exp*1000<Date.now())throw Error('shared_connection_expired');
 const read=async(endpoint: string,body?: unknown)=>{const r=await fetch('https://api.cbsh.edu-set.com'+endpoint,{method:body?'POST':'GET',headers:{Authorization:'Bearer '+token,'x-jwt':token,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(12_000)});if(!r.ok)throw Error('school_http_'+r.status);return r.json()};
 const date=new Date().toLocaleDateString('sv-SE',{timeZone:'Asia/Seoul'});
 const [rooms,raw,health]=await Promise.all([read('/rooms'),read('/selfstudy/search',{date:date+'T00:00:00.000Z'}),fetch('http://127.0.0.1:3000/api/health',{signal:AbortSignal.timeout(5000)}).then(r=>r.ok)]);if(!Array.isArray(rooms)||!rooms.length||!health)throw Error('frontend_or_rooms');
 const {selectSelfStudyEntries,selectStudyRooms}=await import(pathToFileURL(path.join(root,'src/lib/studyLookup.ts')).href);const {loadStudyRooms}=await import(pathToFileURL(path.join(root,'src/lib/studyRoomView.ts')).href);
 const records=selectSelfStudyEntries(raw),views=await loadStudyRooms(selectStudyRooms(rooms),records,id=>read('/rooms/'+encodeURIComponent(id)+'/composition'),3);const failures=views.filter(r=>r.failed).length;
 const expiring=claims.exp*1000-Date.now()<24*3600_000;
 status={status:failures||expiring?'degraded':'healthy',checked_at:new Date().toISOString(),rooms:rooms.length,failed_rooms:failures,connection_expires_at:new Date(claims.exp*1000).toISOString(),detail:failures?'Some room layouts need attention.':expiring?'shared_connection_expiring':'Study records and room layouts verified.'};
}catch(error){const safe=new Set(['active_release','shared_connection_missing','shared_connection_expired','frontend_or_rooms']);status.detail=safe.has(error.message)?error.message:/^school_http_\d+$/.test(error.message)?error.message:'School read or response validation failed.'}
await fs.mkdir(folder,{recursive:true,mode:0o700});const temp=statusFile+'.tmp-'+crypto.randomUUID();await fs.writeFile(temp,JSON.stringify(status)+'\n',{mode:0o600});await fs.rename(temp,statusFile);console.log(JSON.stringify(status));if(status.status!=='healthy')process.exitCode=1;
