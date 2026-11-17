import fs from'node:fs';import path from'node:path';
/** Operator config only. No request-controlled host or customer build setting is consulted. */
export function deploymentBackendOrigin(env:Record<string,string|undefined>=process.env){
 const raw=env.CBGPU_DEPLOY_BACKEND_URL??'http://127.0.0.1:54321';let url:URL;
 try{url=new URL(raw)}catch{throw Error('Invalid private deployment metadata origin')}
 if(raw!==raw.trim()||!['http:','https:'].includes(url.protocol)||!['127.0.0.1','localhost','[::1]'].includes(url.hostname)||url.username||url.password||url.pathname!=='/'||url.search||url.hash)throw Error('Invalid private deployment metadata origin');
 return url.origin;
}
export function deploymentListenPort(env:Record<string,string|undefined>=process.env){
 const raw=env.CBGPU_DEPLOY_LISTEN_PORT??'3510';if(!/^[1-9]\d*$/.test(raw)||Number(raw)<1024||Number(raw)>65535)throw Error('Invalid private deployment listener');return Number(raw);
}
/** Explicit handoffs bind the existing store. They never create a new key or silently start empty. */
export function deploymentStatePaths(here:string,env:Record<string,string|undefined>=process.env){
 const explicit=env.CBGPU_DEPLOY_PRIVATE;let privateRoot:string;
 if(explicit!==undefined){
  if(!explicit||!path.isAbsolute(explicit)||explicit!==explicit.trim())throw Error('Existing absolute deployment private state required');
  try{privateRoot=fs.realpathSync(explicit)}catch{throw Error('Existing deployment private state unavailable')}
 }else privateRoot=path.resolve(here,'../private');
 const data=path.join(privateRoot,'frontend-deployments'),key=path.join(privateRoot,'frontend-deploy-key'),database=path.join(data,'deploy.sqlite');
 if(explicit!==undefined){
  try{if(!fs.statSync(privateRoot).isDirectory()||!fs.statSync(data).isDirectory()||!fs.lstatSync(key).isFile()||fs.statSync(key).size!==32||!fs.lstatSync(database).isFile())throw Error()}catch{throw Error('Existing deployment key, data and database required; refusing empty handoff')}
 }
 return{privateRoot,data,key,database};
}
/** Old immutable releases used an absolute symlink alias; preserve it only when the physical file stays inside this project's existing store. */
export function composeManagedFile(base:string,projectId:string,file:unknown){
 if(typeof file!=='string'||!path.isAbsolute(file)||!/^[-a-f0-9]{36}$/i.test(projectId))throw Error('Invalid Compose stop path');
 try{const root=fs.realpathSync(base),project=fs.realpathSync(path.join(root,projectId)),target=fs.realpathSync(file);if(!project.startsWith(root+path.sep)||!target.startsWith(project+path.sep)||!fs.statSync(target).isFile())throw Error();return target}catch{throw Error('Invalid Compose stop path')}
}
