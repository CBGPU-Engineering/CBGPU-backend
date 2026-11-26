import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {inboxRow,recordNotifications}from'../../volumes/functions/_shared/notifications.ts';
const user='11111111-1111-4111-8111-111111111111';
const src=(name:string)=>fs.readFileSync(new URL(`../../volumes/functions/${name}/index.ts`,import.meta.url),'utf8');

test('inbox rows fit the table rules; anything that cannot be stored is dropped, never sent',()=>{
 assert.equal(inboxRow({user_id:'not-a-uuid',kind:'ok',title:'t'}),null);
 assert.equal(inboxRow({user_id:user,kind:'Bad Kind',title:'t'}),null);
 assert.equal(inboxRow({user_id:user,kind:'ok',title:'   '}),null);
 const row=inboxRow({user_id:user,kind:'selfstudy_reminder',title:'t'.repeat(200),body:'b'.repeat(900),route:'/study?reminder=1',data:{date:'2026-11-26'},dedupe_key:'k'.repeat(300)})!;
 assert.equal(row.p_title.length,120);assert.equal(row.p_body.length,700);assert.equal(row.p_dedupe_key!.length,200);
 assert.equal(row.p_route,'/study?reminder=1');assert.deepEqual(row.p_data,{date:'2026-11-26'});
 assert.equal(inboxRow({user_id:user,kind:'ok',title:'t',route:'https://print.cbgpu.com/manage/manufacturing/print/1'})!.p_route,'https://print.cbgpu.com/manage/manufacturing/print/1');
 for(const route of['https://evil.example/x','http://print.cbgpu.com/x','javascript:alert(1)','//evil.example'])assert.equal(inboxRow({user_id:user,kind:'ok',title:'t',route})!.p_route,null,route);
 assert.deepEqual(inboxRow({user_id:user,kind:'ok',title:'t',data:{big:'x'.repeat(2500)}})!.p_data,{},'oversized data is dropped, the notification is kept');
});

test('recording calls the server-only function with the service key and never throws',async()=>{
 const calls:{url:string;init:RequestInit}[]=[];
 const send=async(url:any,init:any)=>{calls.push({url:String(url),init});return new Response(null,{status:200})};
 const n=await recordNotifications([{user_id:user,kind:'ok',title:'one'},{user_id:user,kind:'ok',title:'two'},{user_id:'bad',kind:'ok',title:'skipped'}],{url:'http://kong:8000',key:'synthetic-service-key',fetch:send as typeof fetch});
 assert.equal(n,2);assert.equal(calls.length,2,'invalid items are never sent');
 assert.equal(calls[0].url,'http://kong:8000/rest/v1/rpc/cbgpu_notify');
 const headers=calls[0].init.headers as Record<string,string>;assert.equal(headers.Authorization,'Bearer synthetic-service-key');assert.equal(headers.apikey,'synthetic-service-key');
 assert.equal(JSON.parse(String(calls[0].init.body)).p_user_id,user);
 // failures (HTTP error, network error) are swallowed and only successes are counted
 let i=0;const flaky=async()=>{i++;if(i===1)throw new Error('network');if(i===2)return new Response('x',{status:500});return new Response(null,{status:200})};
 assert.equal(await recordNotifications([1,2,3].map(k=>({user_id:user,kind:'ok',title:String(k)})),{url:'u',key:'k',fetch:flaky as typeof fetch}),1);
 let called=false;assert.equal(await recordNotifications([{user_id:user,kind:'ok',title:'t'}],{fetch:(async()=>{called=true;return new Response()}) as typeof fetch}),0);assert.equal(called,false,'no server configuration, no request');
});

test('every push producer stores the inbox entry first, even for accounts without a device',()=>{
 const job=src('notify-job-manager');
 const rec=job.indexOf('recordNotifications(targets.map'),dry=job.indexOf('dry_run:true'),noTokens=job.indexOf("reason:'no device tokens'"),send=job.indexOf('sendFcmPush(');
 assert.ok(dry>0&&rec>dry,'a dry run records nothing');assert.ok(rec<noTokens,'managers without a device still get the entry');assert.ok(rec<send);
 const admin=src('notify-admin-event');
 assert.ok(admin.indexOf('recordNotifications(adminIds.map')<admin.indexOf("reason: 'no device tokens'"));
 assert.ok(admin.indexOf("reason: 'test account'")<admin.indexOf('recordNotifications('),'throw-away test signups still create no notification');
 const study=src('selfstudy-reminder');
 assert.ok(study.indexOf('selfstudy_reminder_attempts').valueOf()>0&&study.indexOf("kind: 'selfstudy_reminder'")>study.indexOf('if (claimError)'),'recorded only after the attempt is claimed');
 assert.ok(study.indexOf("kind: 'selfstudy_reminder'")<study.indexOf('Promise.allSettled((profile.device_tokens'),'recorded before the push is attempted');
});
