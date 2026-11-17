import crypto from 'node:crypto';
import {createClient} from '@supabase/supabase-js';
export const sessionCookie='__Host-cbgpu-developer';
const fault=(message,status=400)=>Object.assign(new Error(message),{status});
/** Shared account-only portal authentication. Role lookup remains server-side and fail closed. */
export function developerAuth({db,settings,env,loadPrincipals,audit=((...args: any[])=>{}),onRevoke=((...args: any[])=>{}),scope='developer',dynamicMembers=false}){
 const cookieName=scope==='developer'?sessionCookie:'__Host-cbgpu-'+scope;
 const pendingCookie=cookieName+'-pending';
 const hash=value=>crypto.createHmac('sha256',settings.sessionSecret).update(scope+':'+value).digest('hex');
 const encryption=crypto.createHash('sha256').update('portal-account-tokens:'+scope+':'+settings.sessionSecret).digest();
 // Sliding sessions: every page load extends the idle window, capped by an absolute maximum age.
 const IDLE=Math.max(1,Number(settings.sessionIdleHours)||24)*3600000,MAX_AGE=Math.max(1,Number(settings.sessionMaxDays)||14)*86400000;
 const allowed=new Map(settings.developers.map(d=>[d.email.toLowerCase(),d]));
 let principals=new Map(),healthy=false,refreshing=null;
 const columns=new Set(db.prepare('PRAGMA table_info(sessions)').all().map(r=>r.name));
 for(const [name,type] of [['user_id','TEXT'],['email','TEXT'],['auth_version','TEXT'],['auth_session_id','TEXT'],['auth_tokens','TEXT'],['issued','INTEGER']])if(!columns.has(name))db.exec(`ALTER TABLE sessions ADD COLUMN ${name} ${type}`);
 db.prepare('DELETE FROM sessions WHERE user_id IS NULL OR auth_version IS NULL OR auth_session_id IS NULL').run();
 db.exec('CREATE TABLE IF NOT EXISTS failures(key TEXT PRIMARY KEY,count INTEGER NOT NULL,expires INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS pending_auth(token_hash TEXT PRIMARY KEY,csrf TEXT NOT NULL,payload TEXT NOT NULL,expires INTEGER NOT NULL,attempts INTEGER NOT NULL DEFAULT 0);DELETE FROM pending_auth;');
 const version=member=>hash('account-only-v1:'+member.id);
 const eligible=id=>healthy?principals.get(id):null;
 const memberFor=email=>dynamicMembers?[...principals.values()].find(p=>p.email.toLowerCase()===email.toLowerCase()):allowed.get(email.toLowerCase());
 function alive(id,sid){return eligible(id)?.sessions.some(s=>s.id===sid)}
 function valid(s){const p=eligible(s.user_id),member=memberFor(s.email||'');return s.expires>Date.now()&&p&&member&&member.id===s.user_id&&p.email.toLowerCase()===member.email.toLowerCase()&&s.auth_version===version(member)&&alive(s.user_id,s.auth_session_id)}
 async function refresh(){if(refreshing)return refreshing;refreshing=(async()=>{try{const rows=await loadPrincipals(settings.developers.map(d=>d.id));principals=new Map(rows.map(p=>[p.id,p]));healthy=true;for(const s of db.prepare('SELECT * FROM sessions').all())if(!valid(s)){db.prepare('DELETE FROM sessions WHERE token_hash=?').run(s.token_hash);onRevoke(s.token_hash);audit(s,'session.revoked',s.user_id,'success')}}catch{healthy=false}finally{refreshing=null}})();return refreshing}
 function seal(value){const iv=crypto.randomBytes(12),c=crypto.createCipheriv('aes-256-gcm',encryption,iv);return Buffer.concat([iv,c.update(JSON.stringify(value)),c.final(),c.getAuthTag()]).toString('base64')}
 function open(value){const b=Buffer.from(value,'base64'),c=crypto.createDecipheriv('aes-256-gcm',encryption,b.subarray(0,12));c.setAuthTag(b.subarray(-16));return JSON.parse(Buffer.concat([c.update(b.subarray(12,-16)),c.final()]).toString('utf8'))}
 const readCookie=req=>(req.headers.cookie||'').split(';').map(v=>v.trim()).find(v=>v.startsWith(cookieName+'='))?.slice(cookieName.length+1);
 const cookie=(name,value,seconds)=>`${name}=${value}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=${seconds}`;
 function session(req){const token=readCookie(req);if(!token||!healthy)return null;const s=db.prepare('SELECT * FROM sessions WHERE token_hash=? AND expires>?').get(hash(token),Date.now());return s&&valid(s)?{...s,user:{id:s.user_id,email:s.email,name:eligible(s.user_id).name,verification_method:'cbgpu_account'}}:null}
 function info(req){const s=session(req);return s?{authenticated:true,csrf_token:s.csrf,user:s.user}:{authenticated:false}}
 function limit(req,key,success=false,maximum=7){const ip=String(req.headers['cf-connecting-ip']||req.socket?.remoteAddress||'local'),id=hash(ip+':'+key);const now=Date.now(),old=db.prepare('SELECT * FROM failures WHERE key=?').get(id);if(success){db.prepare('DELETE FROM failures WHERE key=?').run(id);return}if(old&&old.expires>now&&old.count>=maximum)throw fault('Too many attempts. Try again in 15 minutes.',429);db.prepare('INSERT INTO failures(key,count,expires) VALUES(?,1,?) ON CONFLICT(key) DO UPDATE SET count=CASE WHEN expires>? THEN count+1 ELSE 1 END,expires=CASE WHEN expires>? THEN expires ELSE ? END').run(id,now+900000,now,now,now+900000)}
 async function login(req,res,body){
  const email=typeof body.email==='string'?body.email.trim().toLowerCase():'',password=typeof body.password==='string'?body.password:'';
  try{limit(req,'account-global',false,30);limit(req,'account:'+email)}catch(error){audit({email:email.slice(0,254)},'login.account_failed',scope+' portal','denied',{reason:'rate_limited'});throw error}
  await refresh();const member=memberFor(email);
  if(!healthy){audit({email:email.slice(0,254)},'login.account_failed',scope+' portal','failed',{reason:'role_lookup_unavailable'});throw fault('Sign-in service is temporarily unavailable. Please try again shortly.',503)}
  if(!member||!eligible(member.id)||!password||password.length>256||email.length>254){audit({email:email.slice(0,254)},'login.account_failed',scope+' portal','denied',{reason:'workspace_membership_or_invalid_input'});throw fault('Account credentials are incorrect, or workspace access is unavailable.',401)}
  const client=createClient('http://127.0.0.1:54321',env.ANON_KEY,{auth:{persistSession:false,autoRefreshToken:false,detectSessionInUrl:false}});
  const {data,error}=await client.auth.signInWithPassword({email,password});
  if(error||data.user?.id!==member.id||!data.session){audit({user_id:member.id,email},'login.account_failed',scope+' portal','denied',{reason:'account_credentials',code:error?.code||'identity_or_session_mismatch',status:error?.status||null});if(error?.status===429)throw fault('Too many sign-in attempts. Please wait before trying again.',429);if(error&&(error.status>=500||error.name==='AuthRetryableFetchError'))throw fault('Sign-in service is temporarily unavailable. Please try again shortly.',503);throw fault('Account credentials are incorrect, or workspace access is unavailable.',401)}
  const claims=JSON.parse(Buffer.from(data.session.access_token.split('.')[1],'base64url').toString('utf8'));if(!claims.session_id)throw fault('Account sign-in could not be verified.',401);await refresh();if(!alive(member.id,claims.session_id))throw fault('Account sign-in could not be verified.',401);
  const previous=session(req);if(previous){db.prepare('DELETE FROM sessions WHERE token_hash=?').run(previous.token_hash);onRevoke(previous.token_hash)}
  const token=crypto.randomBytes(32).toString('hex'),csrf=crypto.randomBytes(32).toString('hex');
  db.prepare('INSERT INTO sessions(token_hash,csrf,expires,user_id,email,auth_version,auth_session_id,auth_tokens,issued) VALUES(?,?,?,?,?,?,?,?,?)').run(hash(token),csrf,Date.now()+IDLE,member.id,email,version(member),claims.session_id,seal({access_token:data.session.access_token,refresh_token:data.session.refresh_token}),Date.now());
  limit(req,'account:'+email,true);limit(req,'account-global',true);res.setHeader('Set-Cookie',[cookie(cookieName,token,Math.floor(IDLE/1000)),cookie(pendingCookie,'',0)]);audit({user_id:member.id,email},'login.succeeded',scope+' portal','success');return info({headers:{cookie:cookieName+'='+token}});
 }
 function removed(..._args:unknown[]){throw fault('The additional password step has been removed. Sign in with your CBGPU account.',410)}
 function logout(s,res){db.prepare('DELETE FROM sessions WHERE token_hash=?').run(s.token_hash);onRevoke(s.token_hash);res.setHeader('Set-Cookie',[cookie(cookieName,'',0),cookie(pendingCookie,'',0)]);audit(s,'logout',s.user_id,'success');return{ok:true}}
 const timer=setInterval(()=>{void refresh();db.prepare('DELETE FROM failures WHERE expires<?').run(Date.now())},15000);timer.unref();
 /** Called on page loads: push the expiry forward (at most hourly) and refresh the cookie lifetime. */
 function renew(req,res,s){
  const now=Date.now(),issued=s.issued||now,next=Math.min(now+IDLE,issued+MAX_AGE);
  if(next-s.expires<3600000)return false;
  const token=readCookie(req);if(!token)return false;
  db.prepare('UPDATE sessions SET expires=?,issued=? WHERE token_hash=?').run(next,issued,s.token_hash);
  res.appendHeader('Set-Cookie',cookie(cookieName,token,Math.max(1,Math.floor((next-now)/1000))));
  return true;
 }
 /** The browser refreshes its Supabase tokens on its own; keep the sealed server copy current so a fresh WebView (app restart) can restore the session. */
 async function rememberTokens(s,tokens){
  if(!tokens||typeof tokens.access_token!=='string'||typeof tokens.refresh_token!=='string'||tokens.access_token.length>8192||tokens.refresh_token.length>512)throw fault('Invalid session',400);
  let claims;try{claims=JSON.parse(Buffer.from(tokens.access_token.split('.')[1],'base64url').toString('utf8'))}catch{throw fault('Invalid session',400)}
  if(claims.sub!==s.user_id||claims.session_id!==s.auth_session_id||!(claims.exp*1000>Date.now()))throw fault('Session mismatch',409);
  const client=createClient('http://127.0.0.1:54321',env.ANON_KEY,{auth:{persistSession:false,autoRefreshToken:false,detectSessionInUrl:false}});
  const {data,error}=await client.auth.getUser(tokens.access_token);
  if(error||data.user?.id!==s.user_id)throw fault('Session mismatch',409);
  db.prepare('UPDATE sessions SET auth_tokens=? WHERE token_hash=?').run(seal({access_token:tokens.access_token,refresh_token:tokens.refresh_token}),s.token_hash);
  return{ok:true};
 }
 function nativeSession(s){try{if(!s.auth_tokens)return null;const tokens=open(s.auth_tokens),claims=JSON.parse(Buffer.from(tokens.access_token.split('.')[1],'base64url').toString('utf8'));return claims.exp*1000>Date.now()+30000?tokens:null}catch{return null}}
 return{init:refresh,refresh,nativeSession,rememberTokens,renew,session,pending:()=>null,info,login,system:removed,cancel:removed,logout,close:()=>clearInterval(timer)};
}
