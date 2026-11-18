import test from 'node:test';
import assert from 'node:assert/strict';
import {controllerHealth,controllerHealthCommand} from '../controller-health.mts';
test('default follows native user GPU controller independently of disabled old system controller',async()=>{
 let calls=0;
 assert.equal(await controllerHealth(async(file,args,options)=>{calls++;assert.equal(file,'systemctl');assert.deepEqual(args,['--user','is-active','cbgpu-gpu-controller.service']);assert.equal(options.timeout,3000);return {stdout:'active\n'};}),true);
 assert.equal(calls,1);
});
test('inactive, unknown, unavailable bus and invalid config fail closed without legacy fallback',async()=>{
 for(const value of ['inactive','failed','activating','unknown','active\nfailed'])assert.equal(await controllerHealth(async()=>({stdout:value})),false);
 assert.equal(await controllerHealth(async()=>{throw Error('user bus unavailable')}),false);
 let calls=0;assert.equal(await controllerHealth(async()=>{calls++;return{stdout:'active'}},'native;restart'),false);assert.equal(calls,0);
});
test('explicit legacy compatibility mode has only its fixed read-only command',()=>{assert.deepEqual(controllerHealthCommand('legacy'),['is-active','cbgpu-daemon.service']);assert.throws(()=>controllerHealthCommand('/tmp/unit'));});
