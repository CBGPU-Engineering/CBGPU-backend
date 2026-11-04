import fs from 'node:fs';import path from 'node:path';import {fileURLToPath} from 'node:url';import crypto from 'node:crypto';import {DatabaseSync} from 'node:sqlite';import {WebSocket} from '../developer/node_modules/ws/wrapper.mjs';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');const settings=JSON.parse(fs.readFileSync(root+'/private/developer-portal.json'));const token=crypto.randomBytes(32).toString('hex'),csrf=crypto.randomBytes(32).toString('hex'),hash=crypto.createHmac('sha256',settings.sessionSecret).update(token).digest('hex');const db=new DatabaseSync(root+'/private/developer.sqlite');db.prepare('INSERT INTO sessions(token_hash,csrf,expires) VALUES(?,?,?)').run(hash,csrf,Date.now()+600000);const base=process.env.CBGPU_PORTAL_TEST_URL||'http://127.0.0.1:3400',cookie='__Host-cbgpu-developer='+token;let ws;
async function api(route,body,extra={}){const r=await fetch(base+route,{method:'POST',headers:{Cookie:cookie,Origin:base,'X-CSRF-Token':csrf,'Content-Type':'application/json',...extra},body:JSON.stringify(body)});return{r,data:await r.json()}}
try{
 let result=await api('/api/terminal/session',{}, {'X-CSRF-Token':''});if(result.r.status!==403)throw Error('Terminal ticket CSRF failed');
 result=await api('/api/terminal/session',{}, {Cookie:''});if(result.r.status!==401)throw Error('Anonymous terminal ticket exposed');
 result=await api('/api/terminal/session',{});if(!result.r.ok)throw Error('Ticket failed');
 let text='';ws=new WebSocket(base.replace(/^https:/,'wss:').replace(/^http:/,'ws:')+'/terminal?ticket='+result.data.ticket,{headers:{Cookie:cookie,Origin:base}});
 const ready=new Promise((resolve,reject)=>{ws.on('message',chunk=>{const m=JSON.parse(chunk);if(m.type==='ready')resolve();if(m.type==='error')reject(Error(m.message));if(m.type==='output')text+=m.data});ws.on('error',reject)});await ready;
 function send(data){ws.send(JSON.stringify({type:'input',data}))}
 async function until(needle){const end=Date.now()+15000;while(!text.includes(needle)){if(Date.now()>end)throw Error('Missing PTY output: '+needle+'; last output: '+text.slice(-600));await new Promise(r=>setTimeout(r,50))}}
 send("pwd\nls -d backend\ncommand -v codex\ncodex --version\nprintf '\\x50\\x54\\x59\\x5f\\x4f\\x4b\\n'\n");await until('PTY_OK');if(!text.includes('/home/cbgpu/CBGPU-live')||!text.includes('/home/cbgpu/.local/bin/codex'))throw Error('Shell cwd, ls or Codex missing');
 ws.send(JSON.stringify({type:'resize',cols:120,rows:32}));send("python3 -c 'print(\"INPUT_READY\", flush=True); x=input(); print(\"REPLY_\"+x)'\n");await until('INPUT_READY');await new Promise(r=>setTimeout(r,200));send('interactive_ok\n');await until('REPLY_interactive_ok');
 const closed=new Promise(resolve=>ws.once('close',resolve));await api('/api/logout',{});await closed;
 console.log('Real PTY passed: authenticated ticket, CSRF, Bash cwd, ls, installed Codex, terminal resize, interactive input and logout disconnect.');
}finally{ws?.close();db.prepare('DELETE FROM sessions WHERE token_hash=?').run(hash);db.close()}
