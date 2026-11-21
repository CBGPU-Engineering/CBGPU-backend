/** Pinned consumer contract, not a second runtime implementation.
 * CBGPU-web/src/lib/deployAccess.ts at 50b89d5639081c602d525acb995962798f9c5c60.
 * These pure exports are copied verbatim so backend CI can exercise the saved
 * environment without a network dependency on the separate frontend repo.
 */
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
export function scopedAccountMode(env:Readonly<Record<string,string|undefined>>=process.env){
 const values=[env.CBGPU_MULTISCHOOL_V1,env.CBGPU_ACCOUNT_V2];
 for(const value of values)if(value!==undefined&&!['','0','false','1','true'].includes(value))throw Error('Account authorization configuration is unavailable');
 return values.some(value=>value==='1'||value==='true');
}
export function serverCreationState(draft:boolean,v2:boolean,now:string){return v2&&draft?{status:'pending',deploy_requested_at:null}:{status:'approved',deploy_requested_at:now}}
const [, , file, raw, expected]=process.argv;
const env=Object.fromEntries(readFileSync(file,'utf8').split('\n').filter(line=>line.includes('=')).map(line=>{const split=line.indexOf('=');return[line.slice(0,split),line.slice(split+1)];}));
assert.equal(env.CBGPU_ACCOUNT_V2,raw,'configured account mode must reach the actual saved runtime file unchanged');
if(expected==='invalid'){
 assert.throws(()=>scopedAccountMode(env),/configuration is unavailable/);
}else{
 const scoped=scopedAccountMode(env);
 assert.equal(scoped,expected==='true');
 assert.deepEqual(serverCreationState(true,scoped,'fixture-now'),scoped?{status:'pending',deploy_requested_at:null}:{status:'approved',deploy_requested_at:'fixture-now'});
 assert.deepEqual(serverCreationState(false,scoped,'fixture-now'),{status:'approved',deploy_requested_at:'fixture-now'});
}
