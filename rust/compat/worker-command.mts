import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
/** Temporary API-supervisor adapter. Build logic itself lives in the Rust binary. */
export function frontendWorkerCommand({home=os.homedir(),mode=process.env.CBGPU_FRONTEND_WORKER_MODE||'rust',here}: {home?:string;mode?:string;here?:string}={}){
 if(!here)throw Error('Worker source directory is required');
 if(mode!=='rust')throw Error('Unknown frontend build implementation');
 const binary=path.join(home,'.local/lib/cbgpu-rust/workers/cbgpu-front-worker');
 fs.accessSync(binary,fs.constants.X_OK);
 return{command:fs.realpathSync(binary),args:[],implementation:'rust'};
}

/** Native Compose execution, measurement and operator diagnostics. */
export function composeWorkerCommand({home=os.homedir(),mode=process.env.CBGPU_COMPOSE_WORKER_MODE||'rust'}:{home?:string;mode?:string}={}){
 if(mode!=='rust')throw Error('Compose execution requires Rust');
 const binary=path.join(home,'.local/lib/cbgpu-rust/workers/cbgpu-compose-worker');
 fs.accessSync(binary,fs.constants.X_OK);
 return {command:fs.realpathSync(binary),args:[],implementation:'rust'};
}
