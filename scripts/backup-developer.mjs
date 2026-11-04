import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {DatabaseSync,backup} from 'node:sqlite';
const backend=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const destination=path.resolve(process.argv[2]||'');
if(!destination.startsWith(path.join(backend,'backups')+'/')||!fs.statSync(destination).isDirectory())throw Error('Expected an existing backend backup directory');
const folder=path.join(destination,'developer');fs.mkdirSync(folder,{mode:0o700});
for(const name of ['developer-portal.json','developer-database.pgpass','frontend-runtime.env','email-sender-status.json','materials.env']){
 const from=path.join(backend,'private',name);if(!fs.existsSync(from))continue;const to=path.join(folder,name);fs.copyFileSync(from,to);fs.chmodSync(to,0o600);
}
const source=new DatabaseSync(path.join(backend,'private/developer.sqlite'),{readOnly:true});
try{await backup(source,path.join(folder,'developer.sqlite'));fs.chmodSync(path.join(folder,'developer.sqlite'),0o600)}finally{source.close()}
for(const name of ['cbgpu-frontend.service','cbgpu-developer-portal.service']){const from=path.join('/home/cbgpu/.config/systemd/user',name);if(fs.existsSync(from))fs.copyFileSync(from,path.join(folder,name))}
console.log('Developer configuration and consistent SQLite snapshot backed up.');
