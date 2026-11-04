import fs from 'node:fs';import path from 'node:path';import os from 'node:os';import crypto from 'node:crypto';import {fileURLToPath} from 'node:url';import {promisify} from 'node:util';import {execFile,spawn} from 'node:child_process';import {DatabaseSync} from 'node:sqlite';
import {createRequire} from 'node:module';const require=createRequire(new URL('../developer/package.json',import.meta.url));const {createClient}=require('@supabase/supabase-js');
import {developerAuth} from '../developer/auth.mjs';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');const env=Object.fromEntries(fs.readFileSync(path.join(root,'.env'),'utf8').split('\n').filter(l=>l.includes('=')&&!l.startsWith('#')).map(l=>{const i=l.indexOf('=');return[l.slice(0,i),l.slice(i+1)]}));
const admin=createClient('http://127.0.0.1:54321',env.SERVICE_ROLE_KEY,{auth:{persistSession:false,autoRefreshToken:false}});const run=promisify(execFile),dir=fs.mkdtempSync(path.join(root,'private/auth-test-'));let userId,auth;const db=new DatabaseSync(path.join(dir,'auth.sqlite'));db.exec('CREATE TABLE sessions(token_hash TEXT PRIMARY KEY,csrf TEXT,expires INTEGER);CREATE TABLE failures(key TEXT PRIMARY KEY,count INTEGER,expires INTEGER)');
const systemPassword=crypto.randomBytes(20).toString('hex'),salt=crypto.randomBytes(16).toString('hex');
const password=crypto.randomBytes(24).toString('base64url'),email='developer-verification-'+crypto.randomUUID()+'@example.invalid';const jar=new Map();let csrf='';
const request=()=>({headers:{cookie:[...jar].map(([k,v])=>k+'='+v).join(';'),'x-csrf-token':csrf},socket:{remoteAddress:'127.0.0.1'}});
const response={setHeader(name,value){if(name!=='Set-Cookie')return;for(const c of Array.isArray(value)?value:[value]){const first=c.split(';')[0],i=first.indexOf('=');jar.set(first.slice(0,i),first.slice(i+1))}}};
function otp(secret){const alphabet='ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';let bits='';for(const c of secret.replace(/=+$/,''))bits+=alphabet.indexOf(c).toString(2).padStart(5,'0');const key=Buffer.from((bits.match(/.{8}/g)||[]).map(b=>parseInt(b,2)));const counter=Buffer.alloc(8);counter.writeBigUInt64BE(BigInt(Math.floor(Date.now()/30000)));const h=crypto.createHmac('sha1',key).update(counter).digest(),offset=h[19]&15;return String((h.readUInt32BE(offset)&0x7fffffff)%1000000).padStart(6,'0')}
async function loadPrincipals(){const query=`SELECT coalesce(json_agg(json_build_object('id',p.id,'name',p.full_name,'email',u.email,'factors',ARRAY(SELECT id::text FROM auth.mfa_factors WHERE user_id=u.id AND status='verified'),'sessions',coalesce((SELECT json_agg(json_build_object('id',id::text,'aal',aal::text)) FROM auth.sessions WHERE user_id=u.id),'[]'::json))),'[]'::json) FROM public.profiles p JOIN auth.users u ON u.id=p.id WHERE p.id='${userId}' AND p.is_admin IS TRUE AND u.deleted_at IS NULL`;return JSON.parse((await run('docker',['exec','cbgpu-supabase-db','psql','-X','-U','supabase_admin','-d','postgres','-At','-c',query])).stdout)}
try{
 userId=crypto.randomUUID();
 const fixtureSql=`BEGIN; SET LOCAL session_replication_role=replica;
 INSERT INTO auth.users(instance_id,id,aud,role,email,encrypted_password,email_confirmed_at,confirmation_token,recovery_token,email_change_token_new,email_change,raw_app_meta_data,raw_user_meta_data,created_at,updated_at) VALUES('00000000-0000-0000-0000-000000000000','${userId}','authenticated','authenticated','${email}',extensions.crypt('${password}',extensions.gen_salt('bf')),now(),'','','','','{"provider":"email","providers":["email"]}','{"full_name":"CBGPU MFA verification (temporary)"}',now(),now());
 INSERT INTO auth.identities(provider_id,user_id,identity_data,provider,created_at,updated_at) VALUES('${userId}','${userId}','{"sub":"${userId}","email":"${email}","email_verified":true}','email',now(),now());
 INSERT INTO public.profiles(id,full_name,student_id,student_id_school_year,is_admin,is_approved) VALUES('${userId}','CBGPU MFA verification (temporary)','3399',public.cbgpu_school_year(now()),true,true);
 COMMIT;`;
 await new Promise((resolve,reject)=>{const child=spawn('docker',['exec','-i','cbgpu-supabase-db','psql','-X','-U','supabase_admin','-d','postgres','-v','ON_ERROR_STOP=1'],{stdio:['pipe','ignore','pipe']});child.stderr.on('data',()=>{});child.on('error',reject);child.on('exit',code=>code===0?resolve():reject(Error('Temporary auth fixture creation failed')));child.stdin.end(fixtureSql)});
 auth=developerAuth({db,settings:{sessionSecret:crypto.randomBytes(32).toString('hex'),developers:[{id:userId,email,passwordSalt:salt,passwordHash:crypto.scryptSync(systemPassword,salt,64).toString('hex')}]},env,loadPrincipals});await auth.init();
 let rejected=false;try{await auth.login(request(),response,{email,password:'incorrect'})}catch(e){rejected=e.status===401}if(!rejected)throw Error('Incorrect password accepted');
 const verified=await auth.login(request(),response,{email,password});csrf=verified.csrf_token;
 if(!verified.authenticated||verified.system_password_required||!auth.session(request())||auth.pending(request()))throw Error('Single-step account session failed');
 if(verified.user.verification_method!=='cbgpu_account')throw Error('Unexpected authentication method');
 let removed=false;try{await auth.system(request(),response,{password:systemPassword})}catch(e){removed=e.status===410}if(!removed)throw Error('Obsolete additional-password endpoint remains active');
 const tokens=auth.nativeSession(auth.session(request()));if(!tokens?.access_token||!tokens?.refresh_token)throw Error('Native session handoff failed');
 auth.logout(auth.session(request()),response);if(auth.session(request()))throw Error('Logout failed');
 await auth.login(request(),response,{email,password});
 await admin.from('profiles').update({is_admin:false}).eq('id',userId);await auth.refresh();if(auth.session(request()))throw Error('Admin revocation failed');
 console.log('PASS account-only developer authentication: incorrect password denied, single-step session, native handoff, obsolete step disabled, logout and role revocation.');

}finally{auth?.close();db.close();if(userId)await admin.auth.admin.deleteUser(userId);fs.rmSync(dir,{recursive:true,force:true})}
