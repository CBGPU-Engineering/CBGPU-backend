import test from 'node:test';import assert from 'node:assert/strict';import fs from 'node:fs';import path from 'node:path';import os from 'node:os';import http from 'node:http';
import {equipmentPortals} from '../equipment.mjs';
// Fake website that echoes what the gateway forwarded.
async function setup(){
 const upstream=http.createServer((req,res)=>{res.setHeader('content-type','application/json');res.end(JSON.stringify({path:req.url,host:req.headers.host,gate:req.headers['x-cbgpu-portal-gate']??null,publicHost:req.headers['x-cbgpu-public-host']}))});
 await new Promise(r=>upstream.listen(0,'127.0.0.1',r));
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'cbgpu-equip-'));fs.mkdirSync(path.join(root,'private'));
 const portals=equipmentPortals({root,settings:{sessionSecret:'s'.repeat(32),ingestSecret:'i'.repeat(32),developers:[]},env:{ANON_KEY:'k'},run:async()=>({stdout:'[]'}),audit(){},frontendPort:upstream.address().port});
 const front=http.createServer(async(req,res)=>{if(!await portals.handle(req,res,String(req.headers.host).split(':')[0])){res.writeHead(418);res.end('not equipment')}});
 await new Promise(r=>front.listen(0,'127.0.0.1',r));
 // fetch() cannot override Host, so use http.request.
 const call=(host,p,headers={})=>new Promise((resolve,reject)=>{const r=http.request({hostname:'127.0.0.1',port:front.address().port,path:p,method:'GET',headers:{...headers,host}},res=>{let body='';res.on('data',c=>body+=c);res.on('end',()=>resolve({status:res.statusCode,headers:{get:k=>res.headers[k.toLowerCase()]??null},json:async()=>JSON.parse(body)}))});r.on('error',reject);r.end()});
 return{call,close:()=>{front.close();upstream.close();portals.close()}};
}
test('switch off (default): the gateway still stands in front — no session, no access to the app',async()=>{
 delete process.env.CBGPU_EQUIPMENT_PASSTHROUGH;const s=await setup();
 try{
  const r=await s.call('print.cbgpu.com','/manage/manufacturing');assert.equal(r.status,303);assert.equal(r.headers.get('location'),'/');
  const m=await s.call('manufacturing.cbgpu.com','/manage/manufacturing');assert.equal(m.status,303);
  const api=await s.call('print.cbgpu.com','/api/manufacturing/access');assert.equal(api.status,401);
 }finally{s.close()}
});
test('switch on: all three hosts go straight to the website, and a client cannot fake the gate header',async()=>{
 process.env.CBGPU_EQUIPMENT_PASSTHROUGH='1';const s=await setup();
 try{
  for(const host of ['manufacturing.cbgpu.com','print.cbgpu.com','laser.cbgpu.com']){
   const r=await s.call(host,'/manage/manufacturing/requests',{'x-cbgpu-portal-gate':'1'});assert.equal(r.status,200);const body=await r.json();
   assert.equal(body.path,'/manage/manufacturing/requests');assert.equal(body.publicHost,host);assert.equal(body.gate,null,'spoofed gate header must be stripped');
  }
  const other=await s.call('cbgpu.com','/');assert.equal(other.status,418,'non-equipment hosts are not handled by the gateway');
 }finally{delete process.env.CBGPU_EQUIPMENT_PASSTHROUGH;s.close()}
});
