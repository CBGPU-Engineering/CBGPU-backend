/** Read-only school probe. Never use this retry policy for applications/cancellations. */
export async function readSchool(endpoint:string, token:string, body:unknown, deadline:number, transport:typeof fetch=fetch):Promise<unknown>{
 if(!(endpoint==='/rooms'&&body===undefined||/^\/rooms\/[a-zA-Z0-9_-]+\/composition$/.test(endpoint)&&body===undefined||endpoint==='/selfstudy/search'&&body!==undefined))throw Error('school_read_not_allowed');
 for(let attempt=0;attempt<3;attempt++){
  const remaining=deadline-Date.now();
  if(remaining<=0)throw Error('school_timeout');
  let response:Response;
  try{response=await transport('https://api.cbsh.edu-set.com'+endpoint,{method:body===undefined?'GET':'POST',headers:{Authorization:'Bearer '+token,'x-jwt':token,'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(Math.max(1,Math.min(12_000,remaining)))});}
  catch(error){if(attempt<2&&deadline>Date.now())continue;throw Error(error instanceof Error&&['TimeoutError','AbortError'].includes(error.name)?'school_timeout':'school_network');}
  if(!response.ok){if([408,429,502,503,504].includes(response.status)&&attempt<2&&deadline>Date.now())continue;throw Error('school_http_'+response.status);}
  try{return await response.json()}catch{throw Error('school_invalid_json')}
 }
 throw Error('school_network');
}
