import crypto from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import fs from 'node:fs';
import {fileURLToPath} from 'node:url';
import {WebSocketServer, WebSocket} from 'ws';
import pty from 'node-pty';

const allowedOrigins=new Set(['https://developers.cbgpu.com','http://127.0.0.1:3400','http://localhost:3400']);
export function browserTerminal({server,session,cwd,audit=()=>{}}) {
 const tickets=new Map(),connections=new Map();
 const folder=process.env.CBGPU_PORTAL_ROOT?path.join(process.env.CBGPU_PORTAL_ROOT,'private/terminals'):path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../private/terminals');fs.mkdirSync(folder,{recursive:true,mode:0o700});
 const socketPath=path.join(folder,'tmux.sock'),configPath=path.join(path.dirname(fileURLToPath(import.meta.url)),'tmux.conf');
 const sockets=new WebSocketServer({noServer:true,maxPayload:65536,perMessageDeflate:false});
 const deny=(socket,status=403)=>{socket.end(`HTTP/1.1 ${status} Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`)};
 function stop(ws,c,code,reason){try{c.child.kill()}catch{}ws.close(code,reason)}
 function closeSession(hash){for(const [ws,c] of connections)if(c.session===hash)stop(ws,c,1000,'Signed out')}
 function ticket(s){const value=crypto.randomBytes(32).toString('hex');tickets.set(value,{session:s.token_hash,expires:Date.now()+30000});return value}
 server.on('upgrade',(req,socket,head)=>{
  const url=new URL(req.url,'http://localhost');const value=url.searchParams.get('ticket');const t=tickets.get(value);tickets.delete(value);
  const s=session(req);
  if(!['developers.cbgpu.com','127.0.0.1','localhost'].includes((req.headers.host||'').split(':')[0])||url.pathname!=='/terminal'||!allowedOrigins.has(req.headers.origin)||!s||!t||t.expires<Date.now()||t.session!==s.token_hash)return deny(socket);
  if(connections.size>=4||[...connections.values()].filter(c=>c.session===s.token_hash).length>=2)return deny(socket,429);
  sockets.handleUpgrade(req,socket,head,ws=>{
   const shellEnv={};for(const name of ['HOME','USER','LOGNAME','LANG','LC_ALL','XDG_RUNTIME_DIR','DBUS_SESSION_BUS_ADDRESS','DISPLAY','WAYLAND_DISPLAY','SSH_AUTH_SOCK'])if(process.env[name])shellEnv[name]=process.env[name];
   Object.assign(shellEnv,{HOME:os.homedir(),USER:os.userInfo().username,LOGNAME:os.userInfo().username,SHELL:'/bin/bash',TERM:'xterm-256color',COLORTERM:'truecolor',PATH:os.homedir()+'/.local/bin:'+os.homedir()+'/.npm-global/bin:/usr/local/bin:/usr/bin:/bin',CBGPU_BROWSER_TERMINAL:'1'});
   const owner=s.user_id;if(!/^[a-f0-9-]{36}$/.test(owner||'')){ws.close(1008,'Developer identity is required');return}const workspace='developer-'+owner;
   let child;try{child=pty.spawn('/usr/bin/tmux',['-S',socketPath,'-f',configPath,'new-session','-A','-s',workspace,'-c',cwd],{name:'xterm-256color',cols:100,rows:28,cwd,env:shellEnv})}catch{ws.send(JSON.stringify({type:'error',message:'Unable to start the server shell.'}));ws.close(1011);return}
   audit(s,'terminal.attached',workspace,'success');
   connections.set(ws,{session:s.token_hash,child,valid:()=>!!session(req)});
   const send=data=>{if(ws.readyState===WebSocket.OPEN)ws.send(JSON.stringify(data))};
   let paused=false;
   child.onData(data=>{send({type:'output',data});if(ws.bufferedAmount>1048576&&!paused){child.pause();paused=true}});
   child.onExit(({exitCode,signal})=>{send({type:'exit',exitCode,signal});ws.close(1000,'Shell exited')});
   ws.on('message',data=>{try{const m=JSON.parse(data.toString());if(m.type==='input'&&typeof m.data==='string'&&m.data.length<=32768)child.write(m.data);else if(m.type==='resize'&&Number.isInteger(m.cols)&&Number.isInteger(m.rows))child.resize(Math.max(20,Math.min(500,m.cols)),Math.max(5,Math.min(200,m.rows)))}catch{ws.close(1008,'Invalid terminal message')}});
   const flow=setInterval(()=>{if(paused&&ws.bufferedAmount<262144){child.resume();paused=false}},100);flow.unref();
   ws.on('close',()=>{audit(s,'terminal.detached',workspace,'success');clearInterval(flow);connections.delete(ws);try{child.kill()}catch{}});ws.on('error',()=>ws.close());
   send({type:'ready',user:os.userInfo().username,cwd,persistent:true,workspace});
  });
 });
 const expiryTimer=setInterval(()=>{const now=Date.now();for(const [value,t] of tickets)if(t.expires<now)tickets.delete(value);for(const [ws,c] of connections)if(!c.valid())stop(ws,c,1008,'Session expired')},15000);expiryTimer.unref();
 return{ticket,closeSession};
}
