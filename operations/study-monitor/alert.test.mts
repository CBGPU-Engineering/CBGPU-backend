import test from 'node:test';import assert from 'node:assert/strict';
import {decide} from './alert.mts';
const H=3600_000,now=Date.parse('2026-11-09T12:00:00Z');
const at=(t,extra={})=>({status:'healthy',checked_at:new Date(t-60_000).toISOString(),rooms:82,failed_rooms:0,connection_expires_at:new Date(now+200*H).toISOString(),...extra});
const ok=(extra={})=>at(now,extra);
test('healthy and far from expiry sends nothing',()=>{assert.equal(decide(ok(),now,{}).send,false)});
test('no warning more than 24h before expiry',()=>{
 for(const h of [100,60,30,24.5]) assert.equal(decide(ok({connection_expires_at:new Date(now+h*H).toISOString()}),now,{}).send,false);
});
test('24h warning is sent once, then repeats only after 12h',()=>{
 const exp=new Date(now+20*H).toISOString();
 const first=decide(ok({connection_expires_at:exp}),now,{});assert.equal(first.send,true);assert.equal(first.key,'expiring_24h');
 assert.equal(decide(at(now+H,{connection_expires_at:exp}),now+H,{key:'expiring_24h',sentAt:now}).send,false);
 assert.equal(decide(at(now+13*H,{connection_expires_at:exp}),now+13*H,{key:'expiring_24h',sentAt:now}).send,true);
});
test('expiring warning is followed by an immediate expired alert',()=>{
 const exp=new Date(now+1*H).toISOString();
 const d=decide(at(now+2*H,{connection_expires_at:exp}),now+2*H,{key:'expiring_24h',sentAt:now});
 assert.equal(d.send,true);assert.equal(d.key,'connection_expired');assert.equal(d.level,'critical');
});
test('expired connection is critical',()=>{
 const d=decide(ok({status:'unavailable',detail:'shared_connection_expired'}),now,{});assert.equal(d.level,'critical');assert.equal(d.key,'connection_expired');
 const d2=decide(ok({connection_expires_at:new Date(now-H).toISOString()}),now,{});assert.equal(d2.key,'connection_expired');
});
test('missing token and school errors are critical',()=>{
 assert.equal(decide(ok({status:'unavailable',detail:'shared_connection_missing'}),now,{}).key,'connection_missing');
 assert.equal(decide(ok({status:'unavailable',detail:'school_http_401'}),now,{}).key,'school_unavailable');
});
test('degraded rooms warn',()=>{const d=decide(ok({status:'degraded',failed_rooms:3}),now,{});assert.equal(d.key,'rooms_degraded');assert.equal(d.level,'warning')});
test('stale, missing or future-dated probe result is reported',()=>{
 assert.equal(decide(ok({checked_at:new Date(now-20*60_000).toISOString()}),now,{}).key,'probe_stalled');
 assert.equal(decide(null,now,{}).key,'probe_stalled');
 assert.equal(decide(ok({checked_at:new Date(now+3600_000).toISOString()}),now,{}).key,'probe_stalled');
});
test('critical alerts repeat every 3h, recovery is announced once',()=>{
 const s=ok({status:'unavailable',detail:'shared_connection_expired'});
 const bad=t=>at(t,{status:'unavailable',detail:'shared_connection_expired'});
 assert.equal(decide(bad(now+2*H),now+2*H,{key:'connection_expired',sentAt:now}).send,false);
 assert.equal(decide(bad(now+4*H),now+4*H,{key:'connection_expired',sentAt:now}).send,true);
 const rec=decide(ok(),now,{key:'connection_expired',sentAt:now-H});assert.equal(rec.send,true);assert.equal(rec.recovered,true);
 assert.equal(decide(ok(),now,{key:'ok'}).send,false);
});
