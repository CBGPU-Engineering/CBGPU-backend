import {execFileSync} from 'node:child_process';
import {existsSync} from 'node:fs';
const files=execFileSync('git',['ls-files','-z'],{encoding:'utf8'}).split('\0').filter(file=>file&&existsSync(file)&&/\.(?:js|jsx|mjs|cjs|py|pyi|pyc)$/i.test(file));
if(files.length)throw Error('Maintained application sources must use TypeScript or Rust: '+files.join(', '));
console.log('Authored source check passed; TypeScript browser assets and native Rust application code contain no maintained Python or JavaScript sources.');
