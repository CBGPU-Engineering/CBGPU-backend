/** Default-off until account schema and all execution boundaries are approved. */
export function deploymentResourceEnforced(env:Record<string,string|undefined>=process.env){return [env.CBGPU_ACCOUNT_V2,env.CBGPU_MULTISCHOOL_V1].some(value=>value!==undefined&&!['','0','false'].includes(value))}
export async function ownerResourceDecision(owner:string,transport:typeof fetch=fetch):Promise<'ALLOW'|'DENY'|'UNAVAILABLE'>{
 if([process.env.CBGPU_ACCOUNT_V2,process.env.CBGPU_MULTISCHOOL_V1].some(value=>value!==undefined&&!['','0','false','1','true'].includes(value)))return 'UNAVAILABLE';
 if(!deploymentResourceEnforced())return 'ALLOW';
 const key=process.env.SERVICE_ROLE_KEY;if(!/^[a-f0-9-]{36}$/i.test(owner))return 'DENY';if(!key)return 'UNAVAILABLE';
 try{const response=await transport('http://127.0.0.1:54321/rest/v1/rpc/cbgpu_service_can_use_resource',{method:'POST',headers:{apikey:key,Authorization:'Bearer '+key,'Content-Type':'application/json'},body:JSON.stringify({user_id:owner,resource:'school_compute',target_school:'cbsh'}),signal:AbortSignal.timeout(5000)});if(!response.ok)return 'UNAVAILABLE';const result=await response.json();return result===true?'ALLOW':result===false?'DENY':'UNAVAILABLE'}catch{return 'UNAVAILABLE'}
}

export async function ownerCanExecute(owner:string,transport:typeof fetch=fetch):Promise<boolean>{return await ownerResourceDecision(owner,transport)==='ALLOW'}
