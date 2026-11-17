import test from 'node:test';import assert from 'node:assert/strict';
import {buildPushMessage}from'../../volumes/functions/_shared/pushMessage.ts';
const owner='00000000-0000-4000-8000-000000000001';
test('native capability sends only owner-bound data and cannot ask Android to bypass the owner check',()=>{
 const message=buildPushMessage({token:'synthetic-token',platform:'android',user_id:owner,native_kind:'android-native-v1'},'Job received','Private request details','cbgpu_requests',{owner_id:'forged-owner',route:'/study'});
 assert.equal(message.data?.owner_id,owner);assert.equal(message.data?.title,'Job received');assert.equal(message.data?.body,'Private request details');assert.equal(message.data?.route,'/study');
 assert.equal('notification'in message,false);assert.equal('notification'in message.android!,false);assert.equal('webpush'in message,false);assert.equal(message.android?.ttl,'300s');
});
test('legacy Flutter payload retains notification body, data and channel exactly',()=>{
 assert.deepEqual(buildPushMessage({token:'legacy',platform:'android'},'Title','Body','cbgpu_requests',{url:'https://admin.cbgpu.com/admin'}),{token:'legacy',notification:{title:'Title',body:'Body'},data:{url:'https://admin.cbgpu.com/admin'},android:{priority:'HIGH',notification:{channel_id:'cbgpu_requests',sound:'default'}}});
 assert.deepEqual(buildPushMessage({token:'legacy',platform:'ios',native_kind:null},'Title','Body'),{token:'legacy',notification:{title:'Title',body:'Body'}});
});
test('native ownerless/foreign platform capability fails closed, all native data are strings',()=>{
 for(const device of [{token:'t',platform:'android',native_kind:'android-native-v1'},{token:'t',platform:'ios',user_id:owner,native_kind:'android-native-v1'}])assert.throws(()=>buildPushMessage(device,'Title','Body'));
 const message=buildPushMessage({token:'t',platform:'android',user_id:owner,native_kind:'android-native-v1'},'a'.repeat(200),'b'.repeat(2000));assert.equal(message.data!.title.length,120);assert.equal(message.data!.body.length,700);assert.equal(message.data!.route,'/');
});
test('actual shared sender serializes native data-only and never sends a notification fallback',async()=>{
 const {sendFcmPush}=await import('../../volumes/functions/_shared/fcm.ts');const original=globalThis.fetch;let sent:Record<string,any>|null=null;
 globalThis.fetch=async(_url,init)=>{sent=JSON.parse(String(init?.body));return new Response('{}',{status:200});};
 try{await sendFcmPush('synthetic-project','synthetic-access-token',{token:'synthetic-device-token',platform:'android',user_id:owner,native_kind:'android-native-v1'},'Title','Body',undefined,{url:'https://app.cbgpu.com/study'});assert.equal(sent!.message.data.owner_id,owner);assert.equal(sent!.message.data.route,'https://app.cbgpu.com/study');assert.equal('notification'in sent!.message,false);assert.equal('notification'in sent!.message.android,false);}finally{globalThis.fetch=original;}
});
