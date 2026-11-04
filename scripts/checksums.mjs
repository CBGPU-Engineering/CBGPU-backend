import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';
const directory=process.argv[2];if(!directory)throw Error('Pass the release asset directory');
const lines=fs.readdirSync(directory).filter(name=>name!=='SHA256SUMS'&&fs.statSync(path.join(directory,name)).isFile()).sort().map(name=>crypto.createHash('sha256').update(fs.readFileSync(path.join(directory,name))).digest('hex')+'  '+name);
if(!lines.length)throw Error('No assets to checksum');fs.writeFileSync(path.join(directory,'SHA256SUMS'),lines.join('\n')+'\n');
