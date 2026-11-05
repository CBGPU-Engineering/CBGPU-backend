import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {once} from 'node:events';
import {WebSocket} from 'ws';
import {browserTerminal} from '../terminal.mjs';

test('long release paths attach a real shell through the persistent short socket', {timeout:20_000}, async()=>{
 const temporary=fs.mkdtempSync(path.join(os.tmpdir(),'cbgpu-term-'));
 const root=path.join(temporary,'release-'+ 'a'.repeat(90));fs.mkdirSync(root);
 const privatePath=path.join(temporary,'private');fs.mkdirSync(privatePath,{mode:0o700});fs.symlinkSync(privatePath,path.join(root,'private'));
 const original=process.env.CBGPU_PORTAL_ROOT;process.env.CBGPU_PORTAL_ROOT=root;
 const identity={token_hash:randomUUID(),user_id:randomUUID()};
 const server=http.createServer((req,res)=>{res.end('test only')});
 const terminal=browserTerminal({server,session:req=>req.headers.cookie==='test-session=valid'?identity:null,cwd:temporary});
 await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));const port=server.address().port;
 const options={origin:'https://developers.cbgpu.com',headers:{Cookie:'test-session=valid'}};
 let ws;
 try{
  assert.ok(Buffer.byteLength(path.join(root,'private/terminals/tmux.sock'))>108);
  const ticket=terminal.ticket(identity);ws=new WebSocket(`ws://127.0.0.1:${port}/terminal?ticket=${ticket}`,options);
  let output='';
  const completed=new Promise((resolve,reject)=>{
   ws.on('error',reject);ws.on('close',()=>{if(!output.includes('4936'))reject(Error('Shell closed before executing a command: '+output))});
   ws.on('message',data=>{const m=JSON.parse(String(data));if(m.type==='ready')ws.send(JSON.stringify({type:'input',data:'echo $((2468 * 2))\r'}));if(m.type==='output'){output+=m.data;if(/(?:\r\n|\n)4936\r?\n/.test(output))resolve()}});
  });
  await completed;
  assert.ok(fs.existsSync(path.join(privatePath,'terminals/tmux.sock')));
  const reused=new WebSocket(`ws://127.0.0.1:${port}/terminal?ticket=${ticket}`,options);
  const denied=await new Promise(resolve=>{reused.on('unexpected-response',(_req,response)=>{response.resume();resolve(response.statusCode)});reused.on('error',()=>{});});
  assert.equal(denied,403);reused.terminate();
  const invalid=new WebSocket(`ws://127.0.0.1:${port}/terminal?ticket=${terminal.ticket(identity)}`,{...options,origin:'https://example.invalid'});
  const wrongOrigin=await new Promise(resolve=>{invalid.on('unexpected-response',(_req,response)=>{response.resume();resolve(response.statusCode)});invalid.on('error',()=>{});});assert.equal(wrongOrigin,403);invalid.terminate();
 }finally{
  if(ws){ws.close();await Promise.race([once(ws,'close'),new Promise(resolve=>setTimeout(resolve,1000))])}
  try{execFileSync('/usr/bin/tmux',['-S',path.join(privatePath,'terminals/tmux.sock'),'kill-server'],{stdio:'ignore'})}catch{}
  server.closeAllConnections();await new Promise(resolve=>server.close(resolve));
  if(original===undefined)delete process.env.CBGPU_PORTAL_ROOT;else process.env.CBGPU_PORTAL_ROOT=original;
  fs.rmSync(temporary,{recursive:true,force:true});
 }
});
