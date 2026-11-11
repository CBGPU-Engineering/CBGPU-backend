#!/usr/bin/env node
// Local operator job. Secrets never enter command arguments, logs or browser output.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import {parseEnv} from 'node:util';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
export function claims(token){try{const c=JSON.parse(Buffer.from(token.split('.')[1],'base64url'));if(!Number.isFinite(c.exp))throw Error();return c}catch{throw Error('invalid_token')}}
export function shouldRenew(token,now=Date.now()){return claims(token).exp*1000-now<=24*3600000}
export function validateReplacement(oldToken,next,now=Date.now()){
 const a=claims(oldToken),b=claims(next);
 if(a.userId==null||b.userId!==a.userId)throw Error('school_account_mismatch');
 if(b.exp*1000<now+24*3600000||b.exp<=a.exp)throw Error('replacement_not_newer');
}
async function main(){
 const home=os.homedir(),folder=path.join(home,'.local/share/cbgpu-study-monitor');fs.mkdirSync(folder,{recursive:true,mode:0o700});
 const statusFile=path.join(folder,'refresh-status.json');
 const write=(status,extra={})=>{const value={status,checked_at:new Date().toISOString(),...extra};fs.writeFileSync(statusFile+'.tmp',JSON.stringify(value)+'\n',{mode:0o600});fs.renameSync(statusFile+'.tmp',statusFile);console.log(JSON.stringify(value));};
 let phase='configuration';
 try{
 const unit=fs.readFileSync(path.join(home,'.config/systemd/user/cbgpu-frontend.service'),'utf8');const root=unit.match(/^WorkingDirectory=(.+)$/m)?.[1];if(!root)throw Error('active_release_missing');
 const env=parseEnv(fs.readFileSync(path.join(root,'.env.production.local'),'utf8')),require=createRequire(path.join(root,'package.json'));
 const {createClient}=require('@supabase/supabase-js');
 const db=createClient('http://127.0.0.1:54321',env.SUPABASE_SERVICE_ROLE_KEY,{auth:{persistSession:false,autoRefreshToken:false}});
 const {data:row,error}=await db.from('eduset_settings').select('access_token_enc').eq('id',1).maybeSingle();if(error||!row?.access_token_enc)throw Error('shared_connection_missing');
 const key=crypto.createHash('sha256').update(env.EDUSET_TOKEN_SECRET).digest(),bytes=Buffer.from(row.access_token_enc,'base64');
 const dec=crypto.createDecipheriv('aes-256-gcm',key,bytes.subarray(0,12));dec.setAuthTag(bytes.subarray(12,28));const old=Buffer.concat([dec.update(bytes.subarray(28)),dec.final()]).toString();
 if(!shouldRenew(old)&&!process.argv.includes('--verify-login')){write('not_due',{connection_expires_at:new Date(claims(old).exp*1000).toISOString()});return;}
 const credentials=path.join(home,'.config/cbgpu-eduset-refresh.env');
 if(!fs.existsSync(credentials)){write('credentials_required');return;}
 const permissions=fs.statSync(credentials);if(permissions.uid!==process.getuid()||(permissions.mode&0o077))throw Error('credentials_permissions');
 const login=parseEnv(fs.readFileSync(credentials,'utf8'));if(!login.EDUSET_USERNAME||!login.EDUSET_PASSWORD){write('credentials_required');return;}
 let previous={};try{previous=JSON.parse(fs.readFileSync(statusFile,'utf8'))}catch{}
 if(previous.retry_after&&Date.parse(previous.retry_after)>Date.now())return;
 if(process.argv.includes('--check')){write('ready');return;}
 // Persist the cooldown before launching, including across crashes or service timeouts.
 write('attempting',{retry_after:new Date(Date.now()+6*3600000).toISOString()});
 const module=require('@sparticuz/chromium'),chromium=module.default||module,puppeteer=require('puppeteer-core');
 phase='browser_start';
 const browser=await puppeteer.launch({args:chromium.args,executablePath:await chromium.executablePath(),headless:true});
 let next;
 try{
  const page=await browser.newPage();page.setDefaultTimeout(20000);
  phase='login_form';
  await page.goto('https://cbsh.edu-set.com/signin',{waitUntil:'domcontentloaded',timeout:25000});
  await page.waitForSelector('input[name="id"]');await page.type('input[name="id"]',login.EDUSET_USERNAME);await page.type('input[name="password"]',login.EDUSET_PASSWORD);
  // Only accept the site's normal managed verification. No challenge bypass or solver.
  phase='human_verification';
  await page.waitForFunction(()=>!!document.querySelector('input[name="cf-turnstile-response"]')?.value,{timeout:25000});
  phase='login_result';
  await page.click('button[type="submit"]');await page.waitForFunction(()=>!!localStorage.getItem('accessToken'),{timeout:20000});
  next=await page.evaluate(()=>localStorage.getItem('accessToken'));
 }finally{await browser.close()}
 phase='token_validation';
 validateReplacement(old,next);
 const verify=await fetch('https://api.cbsh.edu-set.com/rooms',{headers:{Authorization:'Bearer '+next},signal:AbortSignal.timeout(15000)});if(!verify.ok)throw Error('school_rejected_token');
 const rooms=await verify.json();if(!Array.isArray(rooms)||!rooms.length)throw Error('school_response_invalid');
 if(process.argv.includes('--verify-login')){write('login_verified',{connection_expires_at:new Date(claims(next).exp*1000).toISOString()});return;}
 const iv=crypto.randomBytes(12),enc=crypto.createCipheriv('aes-256-gcm',key,iv),body=Buffer.concat([enc.update(next),enc.final()]);
 const expires=new Date(claims(next).exp*1000).toISOString();
 // Compare-and-swap: never overwrite a token just submitted by an administrator/contributor.
 const result=await db.from('eduset_settings').update({access_token_enc:Buffer.concat([iv,enc.getAuthTag(),body]).toString('base64'),updated_at:new Date().toISOString(),expires_at:expires}).eq('id',1).eq('access_token_enc',row.access_token_enc).select('id');
 if(result.error)throw Error('save_failed');
 write(result.data?.length?'renewed':'superseded',{connection_expires_at:expires});
 }catch(error){const safe=new Set(['invalid_token','school_account_mismatch','replacement_not_newer','active_release_missing','shared_connection_missing','credentials_permissions','school_rejected_token','school_response_invalid','save_failed']);write('needs_attention',{reason:safe.has(error.message)?error.message:phase==='human_verification'?'human_verification_required':'login_or_verification_failed',phase,retry_after:new Date(Date.now()+6*3600000).toISOString()});process.exitCode=1;}
}
if(import.meta.url===pathToFileURL(process.argv[1]||'').href)await main();
