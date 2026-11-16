import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
/** Temporary API-supervisor adapter. Build logic itself lives in the Rust binary. */
export function frontendWorkerCommand({home=os.homedir(),mode=process.env.CBGPU_FRONTEND_WORKER_MODE||'legacy',here}: {home?:string;mode?:string;here?:string}={}){
 if(!here)throw Error('Worker source directory is required');
 if(mode==='legacy')return{command:'python3',args:[path.join(here,'worker.py')],implementation:'legacy-python'};
 if(mode!=='rust')throw Error('Unknown frontend build implementation');
 const binary=path.join(home,'.local/lib/cbgpu-rust/current/cbgpu-front-worker');
 fs.accessSync(binary,fs.constants.X_OK);
 return{command:fs.realpathSync(binary),args:[],implementation:'rust'};
}
