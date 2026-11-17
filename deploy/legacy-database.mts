/** Explicit reviewed reverse bootstrap; authenticated activation receipts permit normal mutable DB restarts. */
import fs from'node:fs';import path from'node:path';import crypto from'node:crypto';
function privateFile(file:string){const fd=fs.openSync(file,fs.constants.O_RDONLY|fs.constants.O_NOFOLLOW),meta=fs.fstatSync(fd);if(!meta.isFile()||meta.uid!==process.getuid?.()||meta.mode&0o077){fs.closeSync(fd);throw Error('Reverse SQLite and manifest must be private operator-owned regular files')}return fd}
function privateJson(file:string,maximum:number){const fd=privateFile(file);try{if(fs.fstatSync(fd).size>maximum)throw Error('Reverse metadata exceeds reviewed bound');return JSON.parse(fs.readFileSync(fd,'utf8'))}finally{fs.closeSync(fd)}}
export function legacyDatabasePath(configured:string|undefined,fallback:string,authenticate:(sealed:string)=>Record<string,any>,expectedFingerprint?:string,seal?:(value:Record<string,any>)=>string){
 if(configured===undefined)return fallback;
 if(!path.isAbsolute(configured)||configured.includes('\0')||configured.length>4096)throw Error('Explicit reverse SQLite path must be absolute');
 if(!/^[a-f0-9]{64}$/.test(expectedFingerprint||''))throw Error('Reviewed current reverse fingerprint required');
 let db:number|undefined;try{
  db=privateFile(configured);const envelope=privateJson(configured+'.manifest.json',1024*1024*1024);
  if(envelope.format!=='cbgpu-deploy-reverse-v1'||typeof envelope.sealed_manifest!=='string')throw Error('Unknown reverse manifest');
  const manifest=authenticate(envelope.sealed_manifest);
  if(manifest.source_fingerprint!==expectedFingerprint)throw Error('Stale reverse fingerprint; original database not selected');
  if(manifest.format!==1||manifest.native_data_root!==fs.realpathSync(path.dirname(fallback))||!/^[a-f0-9]{64}$/.test(manifest.sqlite_sha256||''))throw Error('Reverse source format or artifact-root proof differs');
  const stat=fs.fstatSync(db,{bigint:true}),identity={format:1,path:fs.realpathSync(configured),device:String(stat.dev),inode:String(stat.ino),source_fingerprint:expectedFingerprint,initial_hash:manifest.sqlite_sha256};
  const receiptPath=configured+'.activation.json';let activated=false;
  if(fs.existsSync(receiptPath)){const receipt=privateJson(receiptPath,16384);if(receipt.format!=='cbgpu-legacy-reverse-active-v1'||typeof receipt.sealed_receipt!=='string')throw Error('Invalid reverse activation receipt');const values=authenticate(receipt.sealed_receipt);if(Object.entries(identity).some(([key,value])=>values[key]!==value))throw Error('Reverse activation identity differs');activated=true;}
  const header=Buffer.alloc(16);if(fs.readSync(db,header,0,16,0)!==16||header.toString('binary')!=='SQLite format 3\0')throw Error('Invalid reverse SQLite header');
  if(!activated){const hash=crypto.createHash('sha256'),bytes=Buffer.alloc(65536);let offset=0;for(;;){const read=fs.readSync(db,bytes,0,bytes.length,offset);if(!read)break;hash.update(bytes.subarray(0,read));offset+=read}if(hash.digest('hex')!==manifest.sqlite_sha256)throw Error('Reverse SQLite hash differs; original database not selected');
   if(seal){const receipt={format:'cbgpu-legacy-reverse-active-v1',sealed_receipt:seal(identity)},temporary=receiptPath+'.pending-'+crypto.randomUUID(),fd=fs.openSync(temporary,fs.constants.O_WRONLY|fs.constants.O_CREAT|fs.constants.O_EXCL|fs.constants.O_NOFOLLOW,0o600);try{try{fs.writeFileSync(fd,JSON.stringify(receipt));fs.fsyncSync(fd)}finally{fs.closeSync(fd)}fs.linkSync(temporary,receiptPath)}finally{fs.unlinkSync(temporary)}const directory=fs.openSync(path.dirname(receiptPath),fs.constants.O_RDONLY);try{fs.fsyncSync(directory)}finally{fs.closeSync(directory)}}
  }
  return configured;
 }finally{if(db!==undefined)fs.closeSync(db)}
}
