import test from 'node:test';
import assert from 'node:assert/strict';
import {operationsView} from '../static/operations-view.ts';

test('operations view follows the actual containers response and distinguishes empty from unavailable',()=>{
 const empty=operationsView({containers:[],backups:[],email:{active:'primary',status:'ready'},backup_timer_active:false});
 assert.deepEqual(empty.containers,[]);assert.deepEqual(empty.backups,[]);assert.equal(empty.incomplete,false);assert.equal(empty.timer,false);
 const actual=operationsView({containers:[{name:'cbgpu-web',status:'Up 1 hour',image:'preview'}],backups:[{created_at:'2026-11-24T00:00:00Z',bytes:1024,database:true,storage:true}],email:{}});
 assert.equal(actual.containers?.[0].name,'cbgpu-web');assert.equal(actual.backups?.[0].bytes,1024);
});
test('missing or malformed sections become unavailable without exposing exceptions or dropping valid sections',()=>{
 for(const input of [null,undefined,42,[],{}, {services:[]}, {containers:[null],backups:['invalid'],email:'unavailable'}, {containers:'loading',backups:[{created_at:'now',bytes:NaN}]}]){
  const result=operationsView(input);assert.equal(result.incomplete,true);assert.equal(result.containers,null);assert.equal(result.backups,null);
 }
 const partial=operationsView({containers:[],backups:null,email:{active:'primary'}});
 assert.deepEqual(partial.containers,[]);assert.equal(partial.backups,null);assert.deepEqual(partial.email,{active:'primary'});
});
