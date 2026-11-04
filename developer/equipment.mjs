import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import {DatabaseSync} from 'node:sqlite';
import {developerAuth} from './auth.mjs';
const kinds={'print.cbgpu.com':'print','laser.cbgpu.com':'laser'};
const title=kind=>kind==='print'?'3D Printing':'Laser Cutting';
/** Equipment sessions use the same implementation as developer sessions, in separate scopes/DBs. */
export function equipmentPortals({root,settings,env,run,audit,frontendPort=3000}){
 const portals={};
 for(const kind of ['print','laser']){
  const file=path.join(root,'private',kind+'-portal.sqlite'),db=new DatabaseSync(file);fs.chmodSync(file,0o600);
  db.exec('PRAGMA journal_mode=WAL;CREATE TABLE IF NOT EXISTS sessions(token_hash TEXT PRIMARY KEY,csrf TEXT NOT NULL,expires INTEGER NOT NULL);');
  async function loadPrincipals(){
   const role=kind==='print'?"EXISTS(SELECT 1 FROM public.printer_managers pm WHERE pm.user_id=p.id)":"p.is_laser_cut_manager IS TRUE";
   const query=`SELECT coalesce(json_agg(json_build_object('id',p.id,'name',coalesce(p.full_name,u.email),'email',u.email,'sessions',coalesce((SELECT json_agg(json_build_object('id',s.id::text)) FROM auth.sessions s WHERE s.user_id=u.id AND (s.not_after IS NULL OR s.not_after>now())),'[]'::json))),'[]'::json) FROM public.profiles p JOIN auth.users u ON u.id=p.id WHERE (${role}) AND p.is_approved IS DISTINCT FROM FALSE AND u.deleted_at IS NULL AND (u.banned_until IS NULL OR u.banned_until<=now())`;
   const {stdout}=await run('docker',['exec','cbgpu-supabase-db','psql','-X','-U','supabase_admin','-d','postgres','-At','-v','ON_ERROR_STOP=1','-c',query],{timeout:5000,maxBuffer:2000000});return JSON.parse(stdout.trim());
  }
  const auth=developerAuth({db,settings,env,loadPrincipals,audit,scope:kind+'-manager',dynamicMembers:true});portals[kind]={auth,db};
 }
 function reply(res,status,body){res.writeHead(status,{'Content-Type':'application/json; charset=utf-8'});res.end(JSON.stringify(body))}
 function asset(req,res,name){const file=path.join(root,'developer/static',name),type=name.endsWith('.js')?'text/javascript; charset=utf-8':name.endsWith('.css')?'text/css; charset=utf-8':name.endsWith('.png')?'image/png':'text/html; charset=utf-8';res.writeHead(200,{'Content-Type':type});res.end(req.method==='HEAD'?undefined:fs.readFileSync(file))}
 async function json(req){let body='';for await(const part of req){body+=part;if(Buffer.byteLength(body)>4096)throw Object.assign(Error('Request too large'),{status:413})}try{return JSON.parse(body||'{}')}catch{throw Object.assign(Error('Invalid JSON'),{status:400})}}
 function proxy(req,res,host){
  const upstream=http.request({hostname:'127.0.0.1',port:frontendPort,path:req.url,method:req.method,headers:{...req.headers,host,'x-cbgpu-public-host':host,'x-forwarded-host':host,'x-forwarded-proto':'https'}},response=>{for(const key of ['content-security-policy','x-frame-options','content-type','content-length','content-encoding','transfer-encoding'])res.removeHeader(key);res.writeHead(response.statusCode||502,response.headers);response.pipe(res)});
  upstream.setTimeout(120000,()=>upstream.destroy());upstream.on('error',()=>{if(!res.headersSent)reply(res,502,{error:'Equipment workspace is temporarily unavailable.'});else res.destroy()});req.on('aborted',()=>upstream.destroy());req.pipe(upstream);
 }
 function loginPage(req,res,kind){
  let source=fs.readFileSync(path.join(root,'developer/static/index.html'),'utf8');source=source.slice(0,source.indexOf('  <div id="portal-view"'))+'\n</body></html>';
  source=source.replace('<html lang="en">','<html lang="en" data-portal="'+kind+'">').replaceAll('CBGPU Developers','CBGPU '+title(kind)).replace('CBGPU<span>Developers</span>','CBGPU<span>'+title(kind)+'</span>').replace('developers.cbgpu.com</p>',kind+'.cbgpu.com</p>').replace('Developer sign in</h1>',title(kind)+' sign in</h1>').replace('Access is restricted to authorized developers.','Access is restricted to assigned '+(kind==='print'?'printer':'laser-cutting')+' managers.');
  source=source.replace(/\s*<link rel="stylesheet" href="\/static\/vendor\/xterm.css">/g,'');
  source=source.replace(/\s*<script src="\/static\/(?:vendor\/[^\"]+|terminal-client.js)" defer><\/script>/g,'').replace('/static/app.js','/static/equipment-signin.js');res.writeHead(200,{'Content-Type':'text/html; charset=utf-8'});res.end(req.method==='HEAD'?undefined:source);
 }
 async function handle(req,res,host){
  const kind=kinds[host],legacy=host==='manufacturing.cbgpu.com';if(!kind&&!legacy)return false;
  const url=new URL(req.url,'https://'+host),pathname=url.pathname;
  const assets={'/static/theme.js':'theme.js','/static/style.css':'style.css','/static/assets/logo.png':'assets/logo.png','/static/equipment-signin.js':'equipment-signin.js'};
  if(assets[pathname]&&['GET','HEAD'].includes(req.method)){asset(req,res,assets[pathname]);return true}
  if(legacy){if(pathname!=='/'){res.writeHead(303,{Location:'/'});res.end();return true}asset(req,res,'equipment-index.html');return true}
  const auth=portals[kind].auth,s=auth.session(req);
  if(['/auth/login','/auth/signin','/login'].includes(pathname)&&['GET','HEAD'].includes(req.method)){res.writeHead(303,{Location:'/'});res.end();return true}
  if(pathname==='/portal/api/session'&&req.method==='GET'){reply(res,200,auth.info(req));return true}
  if(pathname==='/portal/api/login'&&req.method==='POST'){
   if(req.headers.origin!=='https://'+host){reply(res,403,{error:'Request origin not allowed'});return true}
   reply(res,200,await auth.login(req,res,await json(req)));return true;
  }
  if(pathname==='/portal/api/logout'&&req.method==='POST'){
   if(!s){reply(res,401,{error:'Please sign in.'});return true}
   if(req.headers.origin!=='https://'+host||req.headers['x-csrf-token']!==s.csrf){reply(res,403,{error:'Invalid request verification'});return true}
   reply(res,200,auth.logout(s,res));return true;
  }
  if(pathname==='/portal/session'&&req.method==='GET'){if(!s){reply(res,401,{error:'Please sign in.'});return true}reply(res,200,{user_id:s.user_id,csrf_token:s.csrf,session:auth.nativeSession(s),equipment_kind:kind});return true}
  if(pathname.startsWith('/portal/')||pathname.startsWith('/api/')&&pathname!=='/api/manufacturing/access'&&pathname!=='/api/timetable'&&!/^\/api\/study\/(?:proxy|auth)(?:\/|$)/.test(pathname)){reply(res,404,{error:'Not found'});return true}
  if(pathname==='/'){
   if(s){res.writeHead(303,{Location:'/manage/manufacturing'});res.end()}else loginPage(req,res,kind);return true;
  }
  const matching=pathname.match(/^\/manage\/manufacturing\/(print|laser)(?:\/|$)/);
  if(matching&&matching[1]!==kind){reply(res,403,{error:'This equipment type belongs to a different workspace.'});return true}
  const allowed=/^\/(?:manage\/manufacturing|_next|img|backend)(?:\/|$)/.test(pathname)||pathname==='/api/manufacturing/access'||pathname==='/api/timetable'||/^\/api\/study\/(?:proxy|auth)(?:\/|$)/.test(pathname)||pathname==='/favicon.ico';
  if(!allowed){reply(res,404,{error:'Not found'});return true}
  if(!s){if(pathname.startsWith('/api/')||pathname.startsWith('/backend/'))reply(res,401,{error:'Please sign in.'});else{res.writeHead(303,{Location:'/'});res.end()}return true}
  proxy(req,res,host);return true;
 }
 return{init:()=>Promise.all(Object.values(portals).map(p=>p.auth.init())),handle,close:()=>Object.values(portals).forEach(p=>{p.auth.close();p.db.close()})};
}
