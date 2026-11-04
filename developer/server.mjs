import {developerAuth} from './auth.mjs';
import {equipmentPortals} from './equipment.mjs';
import {Resend} from 'resend';
import {mailQueue} from './mail.mjs';
import {auditLog} from './audit.mjs';
import {browserTerminal} from './terminal.mjs';
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import { promisify } from 'node:util';
import { execFile, spawn } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';

const ROOT=process.env.CBGPU_PORTAL_ROOT||path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const PRIVATE=path.join(ROOT,'private');const REPO=process.env.CBGPU_PORTAL_REPO||path.dirname(ROOT);const FRONTEND=path.join(REPO,'FrontEnd/cbgpu');
const settings=JSON.parse(fs.readFileSync(path.join(PRIVATE,'developer-portal.json'),'utf8'));
const env=Object.fromEntries(fs.readFileSync(path.join(ROOT,'.env'),'utf8').split('\n').filter(l=>l&&!l.startsWith('#')&&l.includes('=')).map(l=>{const i=l.indexOf('=');return[l.slice(0,i),l.slice(i+1)]}));
const run=promisify(execFile);const scrypt=promisify(crypto.scrypt);const db=new DatabaseSync(path.join(PRIVATE,'developer.sqlite'));
fs.chmodSync(path.join(PRIVATE,'developer.sqlite'),0o600);
db.exec(`PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS sessions(token_hash TEXT PRIMARY KEY,csrf TEXT NOT NULL,expires INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS failures(key TEXT PRIMARY KEY,count INTEGER NOT NULL,expires INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS visits(day TEXT NOT NULL,visitor TEXT NOT NULL,views INTEGER NOT NULL,last_seen INTEGER NOT NULL,PRIMARY KEY(day,visitor));
CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,status TEXT NOT NULL,started_at TEXT,finished_at TEXT,commit_sha TEXT,error TEXT,build_dir TEXT,previous_build TEXT,action TEXT);
CREATE TABLE IF NOT EXISTS state(key TEXT PRIMARY KEY,value TEXT NOT NULL);`);
db.prepare("UPDATE jobs SET status='failed',finished_at=?,error='Developer service restarted during the job; inspect its log before retrying.' WHERE status='running'").run(new Date().toISOString());
const hash=value=>crypto.createHmac('sha256',settings.sessionSecret).update(value).digest('hex');
const same=(a,b)=>{const x=Buffer.from(String(a)),y=Buffer.from(String(b));return x.length===y.length&&crypto.timingSafeEqual(x,y)};
const unit='/home/cbgpu/.config/systemd/user/cbgpu-frontend.service';
const activeDirectory=()=>fs.readFileSync(unit,'utf8').match(/^WorkingDirectory=(.+)$/m)?.[1]||FRONTEND;
const activeBuild=()=>fs.readFileSync(unit,'utf8').match(/^Environment=NEXT_BUILD_DIR=(.+)$/m)?.[1]||'.next';
const sqlValue=value=>`convert_from(decode('${Buffer.from(value).toString('hex')}','hex'),'UTF8')`;
const ident=value=>'"'+String(value).replaceAll('"','""')+'"';
let sqlBusy=0,overviewCache=null,overviewUntil=0;
function fault(message,status=400){return Object.assign(new Error(message),{status})}
function reply(res,status,value){res.writeHead(status,{'Content-Type':'application/json; charset=utf-8'});res.end(JSON.stringify(value))}
async function json(req,limit=30000){let data=[],bytes=0;for await(const chunk of req){bytes+=chunk.length;if(bytes>limit)throw fault('Request body too large',413);data.push(chunk)}try{return JSON.parse(Buffer.concat(data).toString()||'{}')}catch{throw fault('Invalid JSON')}}
function session(req){return auth.session(req)}
const auditStore=auditLog(db);const audit=auditStore.audit;
const operationsConfig=JSON.parse(fs.readFileSync(path.join(PRIVATE,'operations.json'),'utf8'));
const mail=mailQueue({db,secret:settings.sessionSecret,apiKey:env.SMTP_PASS,audit});
async function loadPrincipals(ids){
 if(!ids.every(id=>/^[a-f0-9-]{36}$/.test(id)))throw Error('Invalid developer identifier');
 const allowed=ids.map(id=>"'"+id+"'::uuid").join(',');
 const query=`SELECT coalesce(json_agg(json_build_object('id',p.id,'name',coalesce(p.full_name,u.email),'email',u.email,'factors',ARRAY(SELECT f.id::text FROM auth.mfa_factors f WHERE f.user_id=u.id AND f.status='verified'),'sessions',coalesce((SELECT json_agg(json_build_object('id',a.id::text,'aal',a.aal::text)) FROM auth.sessions a WHERE a.user_id=u.id AND (a.not_after IS NULL OR a.not_after>now())),'[]'::json))),'[]'::json) FROM public.profiles p JOIN auth.users u ON u.id=p.id WHERE p.id IN (${allowed}) AND p.is_admin IS TRUE AND p.is_approved IS TRUE AND u.deleted_at IS NULL AND (u.banned_until IS NULL OR u.banned_until<now())`;
 const {stdout}=await run('docker',['exec','cbgpu-supabase-db','psql','-X','-U','supabase_admin','-d','postgres','-At','-v','ON_ERROR_STOP=1','-c',query],{timeout:5000,maxBuffer:200000});return JSON.parse(stdout.trim());
}
const auth=developerAuth({db,settings,env,loadPrincipals,audit,onRevoke:hash=>terminal.closeSession(hash)});
const equipment=equipmentPortals({root:ROOT,settings,env,run,audit,frontendPort:Number(process.env.CBGPU_ADMIN_FRONTEND_PORT||3000)});

async function sql(query){if(sqlBusy>=2)throw fault('Another database operation is running. Try again shortly.',429);sqlBusy++;try{const command=`SELECT developer_console.run_query(${sqlValue(query)});`;await run('docker',['cp',path.join(PRIVATE,'developer-database.pgpass'),'cbgpu-supabase-db:/tmp/cbgpu-developer.pgpass']);const {stdout}=await run('docker',['exec','-e','PGPASSFILE=/tmp/cbgpu-developer.pgpass','cbgpu-supabase-db','psql','-X','-h','127.0.0.1','-U','cbgpu_developer','-d','postgres','-A','-t','-v','ON_ERROR_STOP=1','-c',command],{timeout:12000,maxBuffer:4*1024*1024});return JSON.parse(stdout.trim())}catch(e){throw fault((e.stderr||e.message||'Database operation failed').split('\n').find(l=>l.includes('ERROR:'))?.replace(/^.*ERROR:\s*/,'')||'Database operation failed or timed out',400)}finally{sqlBusy--}}
let tableCache=[],tableUntil=0;
async function tables(){if(tableUntil>Date.now())return tableCache;const result=await sql(`SELECT n.nspname AS schema,c.relname AS name,greatest(c.reltuples,0)::bigint AS row_estimate,(SELECT json_agg(json_build_object('name',a.attname,'type',format_type(a.atttypid,a.atttypmod),'nullable',NOT a.attnotnull,'primary_key',exists(SELECT 1 FROM pg_index i WHERE i.indrelid=c.oid AND i.indisprimary AND a.attnum=ANY(i.indkey))) ORDER BY a.attnum) FROM pg_attribute a WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped) AS columns FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname IN ('public','developer_workspace') AND c.relkind='r' ORDER BY 1,2`);tableCache=result.rows;tableUntil=Date.now()+10000;return tableCache}
async function table(schema,name){const found=(await tables()).find(t=>t.schema===schema&&t.name===name);if(!found)throw fault('Table not available',404);return found}
async function gitInfo(){try{const [commit,branch,dirty]=await Promise.all([run('git',['rev-parse','HEAD'],{cwd:activeDirectory()}),run('git',['branch','--show-current'],{cwd:activeDirectory()}),run('git',['status','--porcelain','--untracked-files=no'],{cwd:activeDirectory()})]);return{commit:commit.stdout.trim(),branch:branch.stdout.trim()||'detached checkout',dirty:!!dirty.stdout.trim(),active_build:activeBuild()}}catch{return{active_build:activeBuild()}}}
let previousCpu=os.cpus();let cpuPercent=0;setInterval(()=>{const current=os.cpus();let busy=0,total=0;for(let i=0;i<current.length;i++){const before=previousCpu[i]?.times||current[i].times,after=current[i].times;const all=Object.keys(after).reduce((sum,k)=>sum+after[k]-before[k],0);total+=all;busy+=all-(after.idle-before.idle)}cpuPercent=total?100*busy/total:0;previousCpu=current},2000).unref();
async function healthy(url,headers={}){try{const r=await fetch(url,{headers,signal:AbortSignal.timeout(3000)});await r.body?.cancel();return r.ok}catch{return false}}
async function overview(){if(overviewUntil>Date.now())return overviewCache;const [frontend,backend,database,controller,deployment,materials]=await Promise.all([healthy('http://127.0.0.1:3000'),healthy('http://127.0.0.1:54321/auth/v1/health',{apikey:env.ANON_KEY}),run('docker',['exec','cbgpu-supabase-db','pg_isready','-U','postgres'],{timeout:3000}).then(()=>true,()=>false),run('systemctl',['is-active','cbgpu-daemon.service'],{timeout:3000}).then(r=>r.stdout.trim()==='active',()=>false),gitInfo(),healthy('http://127.0.0.1:8421/health')]);const today=new Date().toISOString().slice(0,10);const trafficToday=db.prepare('SELECT coalesce(sum(views),0) AS views,count(*) AS visitors FROM visits WHERE day=?').get(today);const days=db.prepare("SELECT day AS date,sum(views) AS views,count(*) AS visitors FROM visits WHERE day>=date('now','-6 days') GROUP BY day ORDER BY day").all();const disk=fs.statfsSync('/home/cbgpu');overviewCache={health:{frontend,backend,database,controller,materials},system:{cpu_percent:cpuPercent,memory_percent:100*(1-os.freemem()/os.totalmem()),disk_percent:100*(1-disk.bavail/disk.blocks),uptime_seconds:os.uptime()},traffic:{available:!!db.prepare("SELECT value FROM state WHERE key='tracking_since'").get(),today_views:trafficToday.views,today_visitors:trafficToday.visitors,last_7_days:days,tracking_since:db.prepare("SELECT value FROM state WHERE key='tracking_since'").get()?.value||null},deployment,updated_at:new Date().toISOString()};overviewUntil=Date.now()+5000;return overviewCache}
function job(id){const row=db.prepare('SELECT * FROM jobs WHERE id=?').get(id);if(!row)throw fault('Deployment job not found',404);return{...row,commit:row.commit_sha}}
const logPath=id=>path.join(PRIVATE,'deploy-'+id+'.log');
function safeLog(text){for(const value of [settings.sessionSecret,settings.ingestSecret,...Object.entries(env).filter(([k,v])=>/PASSWORD|SECRET|KEY|PASS$/.test(k)&&v.length>10).map(([,v])=>v)])if(value)text=text.replaceAll(value,'[redacted]');return text.replace(/(re_[A-Za-z0-9_]{15,}|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)/g,'[redacted]')}
const logSources={frontend:{label:'Frontend',command:'journalctl',args:['--user','-u','cbgpu-frontend.service','-n','150','--no-pager','-o','short-iso']},auth:{label:'Authentication',command:'docker',args:['logs','--tail','150','--timestamps','cbgpu-supabase-auth']},functions:{label:'Edge functions',command:'docker',args:['logs','--tail','150','--timestamps','cbgpu-supabase-edge-functions']},database:{label:'Postgres',command:'docker',args:['logs','--tail','150','--timestamps','cbgpu-supabase-db']}};
async function operations(){
 const [containers,timer]=await Promise.all([run('docker',['ps','--filter','name=cbgpu-','--format','{{json .}}'],{timeout:5000,maxBuffer:100000}).then(r=>r.stdout.trim().split('\n').filter(Boolean).map(line=>{const c=JSON.parse(line);return{name:c.Names,status:c.Status,image:c.Image}}),()=>[]),run('systemctl',['--user','is-active','cbgpu-backend-backup.timer'],{timeout:3000}).then(r=>r.stdout.trim()==='active',()=>false)]);
 const backups=fs.readdirSync(path.join(ROOT,'backups'),{withFileTypes:true}).filter(d=>d.isDirectory()&&/^\d{8}T\d{6}Z$/.test(d.name)).sort((a,b)=>b.name.localeCompare(a.name)).slice(0,7).map(d=>{const dir=path.join(ROOT,'backups',d.name);const files=fs.readdirSync(dir);return{id:d.name,created_at:fs.statSync(dir).mtime.toISOString(),bytes:files.reduce((n,f)=>n+fs.statSync(path.join(dir,f)).size,0),database:files.includes('database.dump'),storage:files.includes('storage.tar.gz')}});
 let email=null;try{const status=JSON.parse(fs.readFileSync(path.join(PRIVATE,'email-sender-status.json'),'utf8'));email={active:status.active,requested:status.requested,status:status.status}}catch{}
 let offsite=null,monitor=null;try{offsite=JSON.parse(fs.readFileSync(path.join(PRIVATE,'offsite-status.json'),'utf8'));offsite={configured:true,status:offsite.verified_at?'verified':'uploaded',last_backup_at:offsite.created_at,last_success_at:offsite.verified_at}}catch{}try{const r=await fetch(operationsConfig.worker_url+'/status',{headers:{Authorization:'Bearer '+operationsConfig.ops_key},signal:AbortSignal.timeout(5000)});const status=await r.json();monitor={configured:true,enabled:true,status:status.monitor?.incident?'incident':'healthy',last_checked_at:status.monitor?.checked_at,message:'External Cloudflare monitor; alerts go to both developers.'}}catch{monitor={configured:true,status:'unavailable'}}
 return{offsite,monitor,containers,backups,backup_timer_active:timer,email,log_sources:Object.entries(logSources).map(([id,s])=>({id,label:s.label})),updated_at:new Date().toISOString()};
}
function proxyAdmin(req,res){
 const upstream=http.request({hostname:'127.0.0.1',port:Number(process.env.CBGPU_ADMIN_FRONTEND_PORT||3000),path:req.url,method:req.method,headers:{...req.headers,host:'admin.cbgpu.com','x-forwarded-host':'admin.cbgpu.com','x-forwarded-proto':'https'}},response=>{
  for(const key of ['content-security-policy','x-frame-options','content-type','content-length','content-encoding','transfer-encoding'])res.removeHeader(key);
  res.writeHead(response.statusCode||502,response.headers);response.pipe(res);
 });upstream.setTimeout(120000,()=>upstream.destroy());upstream.on('error',()=>{if(!res.headersSent)reply(res,502,{error:'Admin service is temporarily unavailable'});else res.destroy()});req.on('aborted',()=>upstream.destroy());req.pipe(upstream);
}
const server=http.createServer(async(req,res)=>{res.setHeader('Strict-Transport-Security','max-age=31536000');res.setHeader('Cache-Control','no-store');res.setHeader('X-Robots-Tag','noindex, nofollow, noarchive');res.setHeader('X-Content-Type-Options','nosniff');res.setHeader('X-Frame-Options','DENY');res.setHeader('Referrer-Policy','no-referrer');res.setHeader('Content-Security-Policy',"default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'");try{const host=(req.headers.host||'').split(':')[0];if(!['developers.cbgpu.com','admin.cbgpu.com','print.cbgpu.com','laser.cbgpu.com','manufacturing.cbgpu.com','127.0.0.1','localhost'].includes(host))throw fault('Host not allowed',403);if(await equipment.handle(req,res,host))return;const url=new URL(req.url,'http://localhost');const adminHost=host==='admin.cbgpu.com';const portalApi=adminHost&&url.pathname.startsWith('/portal/api/');const route=portalApi?url.pathname.slice(7):url.pathname;
 if(adminHost&&route==='/portal/session'){const s=session(req);if(!s)throw fault('Please sign in',401);reply(res,200,{user_id:s.user_id,csrf_token:s.csrf,session:auth.nativeSession(s)});return}
 if(adminHost&&!portalApi&&route!=='/'&&!route.startsWith('/static/')){if(!session(req)){if(route.startsWith('/api/')||route.startsWith('/backend/'))throw fault('Please sign in',401);res.writeHead(303,{Location:'/'});res.end();return}proxyAdmin(req,res);return}
 if(adminHost&&route==='/'&&session(req)){res.writeHead(303,{Location:'/admin'});res.end();return}
 if(route==='/webhooks/resend'&&req.method==='POST'){if(!operationsConfig.resend_webhook_secret)throw fault('Webhook is not configured',503);let bytes=0,chunks=[];for await(const chunk of req){bytes+=chunk.length;if(bytes>100000)throw fault('Webhook too large',413);chunks.push(chunk)}const headers={id:req.headers['svix-id']||req.headers['webhook-id'],timestamp:req.headers['svix-timestamp']||req.headers['webhook-timestamp'],signature:req.headers['svix-signature']||req.headers['webhook-signature']};let event;try{event=new Resend(env.SMTP_PASS).webhooks.verify({payload:Buffer.concat(chunks).toString('utf8'),headers,webhookSecret:operationsConfig.resend_webhook_secret})}catch{throw fault('Invalid webhook signature',401)}reply(res,200,mail.webhook(event,headers.id));return}
 if(route==='/health/ready'){const config=JSON.parse(fs.readFileSync(path.join(PRIVATE,'operations.json'),'utf8'));if(!same((req.headers.authorization||'').replace(/^Bearer /,''),config.monitor_key))throw fault('Forbidden',403);const status=await overview();let restoreFailed=false;try{restoreFailed=!!JSON.parse(fs.readFileSync(path.join(PRIVATE,'offsite-status.json'),'utf8')).restore_error}catch{}const good=Object.values(status.health).every(Boolean)&&status.system.disk_percent<95&&!restoreFailed;reply(res,good?200:503,{ok:good,health:status.health});return}
 if(route==='/health'){reply(res,200,{ok:true});return}
 if(route==='/internal/traffic'&&req.method==='POST'){if(!same(req.headers['x-cbgpu-ingest-key']||'',settings.ingestSecret))throw fault('Forbidden',403);const body=await json(req);if(!/^[a-f0-9]{64}$/.test(body.visitor||''))throw fault('Invalid visitor identifier');const now=Date.now(),day=new Date().toISOString().slice(0,10);db.prepare('INSERT INTO visits(day,visitor,views,last_seen) VALUES(?,?,1,?) ON CONFLICT(day,visitor) DO UPDATE SET views=views+CASE WHEN ?>last_seen+1000 THEN 1 ELSE 0 END,last_seen=?').run(day,body.visitor,now,now,now);db.prepare("INSERT OR IGNORE INTO state(key,value) VALUES('tracking_since',?)").run(new Date().toISOString());reply(res,200,{ok:true});return}
 if(route.startsWith('/internal/mail/')&&req.method==='POST'){if(!same((req.headers.authorization||'').replace(/^Bearer /,''),operationsConfig.mail_key))throw fault('Forbidden',403);const body=await json(req,600000);if(route==='/internal/mail/emails')reply(res,200,mail.enqueue(body,req.headers['idempotency-key']));else if(route==='/internal/mail/emails/batch'&&Array.isArray(body)&&body.length<=50)reply(res,200,{data:body.map((b,i)=>mail.enqueue(b,req.headers['idempotency-key']?req.headers['idempotency-key']+':'+i:null))});else throw fault('Unknown mail route',404);return}
 if(route==='/api/session'&&req.method==='GET'){reply(res,200,auth.info(req));return}
 if(['/api/login','/api/login/system','/api/login/cancel'].includes(route)&&req.method==='POST'){
  if(!['https://developers.cbgpu.com','https://admin.cbgpu.com','http://127.0.0.1:3400','http://localhost:3400'].includes(req.headers.origin||''))throw fault('Request origin not allowed',403);
  const body=await json(req,4096);reply(res,200,route==='/api/login'?await auth.login(req,res,body):route==='/api/login/system'?await auth.system(req,res,body):auth.cancel(req,res));return;
 }
 if(route.startsWith('/api/mfa/'))throw fault('Additional sign-in verification is disabled.',410);
 if(route.startsWith('/api/')){const s=session(req);if(!s)throw fault('Please sign in',401);req.auditActor=s;if(req.method!=='GET'){if(!same(req.headers['x-csrf-token']||'',s.csrf))throw fault('Invalid CSRF token',403);if(!['https://developers.cbgpu.com','https://admin.cbgpu.com','http://127.0.0.1:3400','http://localhost:3400'].includes(req.headers.origin||''))throw fault('Request origin not allowed',403)}
 if(route==='/api/logout'&&req.method==='POST'){reply(res,200,auth.logout(s,res));return}
 if(route==='/api/audit'&&req.method==='GET'){reply(res,200,{events:auditStore.events(url.searchParams.get('limit'))});return}
 if(route==='/api/account'&&req.method==='GET'){reply(res,200,{user:s.user,verification_method:'cbgpu_account',developers:settings.developers.map(d=>({email:d.email})),recovery_kit_available:fs.existsSync(path.join(PRIVATE,'recovery-kit.json'))});return}

 if(route==='/api/emails'&&req.method==='GET'){reply(res,200,mail.list());return}
 if(route==='/api/emails/retry'&&req.method==='POST'){const body=await json(req);reply(res,200,mail.retry(body.id,s));return}
 if(route==='/api/recovery-kit'&&req.method==='GET'){if(s.expires-7.75*3600000<Date.now())throw fault('A recent sign-in is required',403);const kit=JSON.parse(fs.readFileSync(path.join(PRIVATE,'recovery-kit.json'),'utf8'));res.setHeader('Content-Disposition','attachment; filename="cbgpu-recovery-kit.json"');audit(s,'backup.recovery_kit_downloaded','offsite backup','success');reply(res,200,kit);return}
 if(route==='/api/overview'&&req.method==='GET'){reply(res,200,await overview());return}
 if(route==='/api/operations'&&req.method==='GET'){reply(res,200,await operations());return}
 if(route==='/api/logs'&&req.method==='GET'){const source=logSources[url.searchParams.get('source')];if(!source)throw fault('Unknown log source');try{const r=await run(source.command,source.args,{timeout:8000,maxBuffer:300000});reply(res,200,{log:safeLog(r.stdout+r.stderr),updated_at:new Date().toISOString()})}catch{throw fault('This service log is currently unavailable',503)}return}
 if(route==='/api/tables'&&req.method==='GET'){reply(res,200,{tables:await tables()});return}
 if(route==='/api/table'&&req.method==='GET'){
 const t=await table(url.searchParams.get('schema')||'public',url.searchParams.get('name'));
 const integer=(name,fallback,max)=>{const n=Number(url.searchParams.get(name)??fallback);if(!Number.isSafeInteger(n)||n<0||n>max)throw fault('Invalid '+name);return n};
 const limit=Math.max(1,integer('limit',50,100)),offset=integer('offset',0,1000000);
 const sort=url.searchParams.get('sort')||t.columns.find(c=>c.primary_key)?.name||t.columns[0].name;
 if(!t.columns.some(c=>c.name===sort))throw fault('Unknown sort column');
 const order=url.searchParams.get('direction')==='desc'?'DESC':'ASC';
 const search=(url.searchParams.get('search')||'').slice(0,200);
 const where=search?` WHERE strpos(lower(to_jsonb(r)::text),lower(${sqlValue(search)}))>0`:'';
 const target=ident(t.schema)+'.'+ident(t.name);
 const result=await sql(`SELECT r.* FROM ${target} r${where} ORDER BY ${ident(sort)} ${order} NULLS LAST LIMIT ${limit} OFFSET ${offset}`);
 const total=await sql(`SELECT count(*)::bigint AS count FROM ${target} r${where}`);
 reply(res,200,{rows:result.rows,columns:t.columns,total:Number(total.rows[0].count)});return}

 if(route==='/api/sql'&&req.method==='POST'){const body=await json(req);if(typeof body.query!=='string'||!body.query.trim()||Buffer.byteLength(body.query)>20000)throw fault('Enter a query up to 20 KB');const start=performance.now();const result=await sql(body.query);tableUntil=0;audit(s,'database.sql','SQL editor','success',{statement:body.query.trim().split(/\s+/)[0].toUpperCase(),query_sha256:crypto.createHash('sha256').update(body.query).digest('hex'),row_count:result.row_count});reply(res,200,{...result,duration_ms:Math.round(performance.now()-start)});return}
 if(route==='/api/table/update'&&req.method==='POST'){const body=await json(req);const t=await table(body.schema,body.table);const keys=t.columns.filter(c=>c.primary_key).map(c=>c.name),values=Object.keys(body.values||{});if(!keys.length||!body.primary_key||keys.some(k=>body.primary_key[k]===undefined)||!values.length||values.some(k=>!t.columns.some(c=>c.name===k)||keys.includes(k)))throw fault('A complete primary key and editable columns are required');const target=ident(t.schema)+'.'+ident(t.name);const record=`jsonb_populate_record(NULL::${target},${sqlValue(JSON.stringify(body.values))}::jsonb)`;const pkRecord=`jsonb_populate_record(NULL::${target},${sqlValue(JSON.stringify(body.primary_key))}::jsonb)`;const query=`UPDATE ${target} SET ${values.map(k=>ident(k)+'=('+record+').'+ident(k)).join(',')} WHERE ${keys.map(k=>ident(k)+' IS NOT DISTINCT FROM ('+pkRecord+').'+ident(k)).join(' AND ')} RETURNING *`;const result=await sql(query);if(result.row_count!==1)throw fault('The row was not updated',409);audit(s,'database.row_updated',t.schema+'.'+t.name,'success',{columns:values,primary_key:body.primary_key});reply(res,200,{ok:true});return}
 if(route==='/api/deployments'&&req.method==='GET'){reply(res,200,{jobs:db.prepare('SELECT *,commit_sha AS "commit" FROM jobs ORDER BY started_at DESC LIMIT 30').all()});return}
 if(/^\/api\/deployments\/[a-f0-9-]+$/.test(route)&&req.method==='GET'){const id=route.split('/').pop();const j=job(id);let log=fs.existsSync(logPath(id))?fs.readFileSync(logPath(id),'utf8').slice(-100000):'';reply(res,200,{job:j,log:safeLog(log)});return}
 if(route==='/api/deploy')throw fault('Deployments are read-only. Deploy from the server terminal outside this portal.',405);
 if(route==='/api/terminal/session'&&req.method==='POST'){reply(res,200,{ticket:terminal.ticket(s)});return}
 if(route==='/api/terminal')throw fault('Use the interactive terminal connection.',410);

 throw fault('API route not found',404)}
 if(req.method!=='GET'&&req.method!=='HEAD')throw fault('Method not allowed',405);let file;if(route==='/')file=path.join(ROOT,'developer/static/index.html');else if(['/static/app.js','/static/theme.js','/static/style.css','/static/terminal-client.js','/static/vendor/xterm.js','/static/vendor/addon-fit.js','/static/vendor/xterm.css','/static/assets/logo.png'].includes(route))file=path.join(ROOT,'developer',route.slice(1));else throw fault('Not found',404);let contents=fs.readFileSync(file);if(adminHost&&route==='/'){contents=Buffer.from(contents.toString().replaceAll('CBGPU Developers','CBGPU Admin').replace('CBGPU<span>Developers</span>','CBGPU<span>Admin</span>').replace('developers.cbgpu.com</p>','admin.cbgpu.com</p>').replace('Developer sign in</h1>','Admin sign in</h1>').replace('Access is restricted to authorized developers.','Access is restricted to authorized administrators.'))}res.setHeader('Content-Type',file.endsWith('.js')?'text/javascript; charset=utf-8':file.endsWith('.css')?'text/css; charset=utf-8':file.endsWith('.png')?'image/png':'text/html; charset=utf-8');res.writeHead(200);res.end(req.method==='HEAD'?undefined:contents)
 }catch(e){if(req.auditActor&&req.method!=='GET')audit(req.auditActor,'operation.failed',new URL(req.url,'http://localhost').pathname,'failed',{status:e.status||500});reply(res,e.status||500,{error:e.status?e.message:'Operation failed. Check the server logs.'})}
});
const terminal=browserTerminal({server,session,cwd:REPO,audit});
await Promise.all([auth.init(),equipment.init()]);
const port=Number(process.env.CBGPU_PORTAL_PORT||3400);server.listen(port,'127.0.0.1',()=>console.log('CBGPU portal listening on loopback:'+port));
setInterval(()=>{const now=Date.now();db.prepare('DELETE FROM sessions WHERE expires<?').run(now);db.prepare('DELETE FROM failures WHERE expires<?').run(now);db.prepare("DELETE FROM visits WHERE day<date('now','-29 days')").run()},600000).unref();
