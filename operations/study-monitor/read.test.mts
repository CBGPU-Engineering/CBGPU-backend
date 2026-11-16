import test from 'node:test';import assert from 'node:assert/strict';import {readSchool} from './read.mts';
const probe=(transport:typeof fetch)=>readSchool('/selfstudy/search','private-test-token',{date:'2026-11-16'},Date.now()+5000,transport);
test('read-only transient network failure retries and validates JSON',async()=>{let calls=0;assert.deepEqual(await probe((async()=>{if(++calls<3)throw new TypeError('fetch failed');return Response.json([])}) as typeof fetch),[]);assert.equal(calls,3)});
test('authorization rejection is never retried or hidden',async()=>{let calls=0;await assert.rejects(probe((async()=>{calls++;return new Response('',{status:401})}) as typeof fetch),/school_http_401/);assert.equal(calls,1)});
test('persistent network failures remain unavailable',async()=>{let calls=0;await assert.rejects(probe((async()=>{calls++;throw new TypeError('sensitive URL')}) as typeof fetch),/^Error: school_network$/);assert.equal(calls,3)});
test('invalid JSON is distinct from transport failure',async()=>{await assert.rejects(probe((async()=>new Response('not json')) as typeof fetch),/school_invalid_json/)});
test('upstream transient HTTP failures retry',async()=>{let calls=0;assert.deepEqual(await probe((async()=>++calls===1?new Response('',{status:503}):Response.json({ok:true})) as typeof fetch),{ok:true});assert.equal(calls,2)});
test('expired probe budget makes no upstream call',async()=>{let called=false;await assert.rejects(readSchool('/rooms','x',undefined,Date.now()-1,(async()=>{called=true;return Response.json([])}) as typeof fetch),/school_timeout/);assert.equal(called,false)});

test('probe cannot retry or send school application writes',async()=>{let called=false;await assert.rejects(readSchool('/selfstudy','x',{seatId:1},Date.now()+1000,(async()=>{called=true;return Response.json([])}) as typeof fetch),/school_read_not_allowed/);assert.equal(called,false)});
