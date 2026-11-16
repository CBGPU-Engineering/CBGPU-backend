import {execFileSync} from 'node:child_process';
import {existsSync} from 'node:fs';
const files=execFileSync('git',['ls-files','-z'],{encoding:'utf8'}).split('\0').filter(file=>file&&existsSync(file)&&/\.(?:js|jsx|mjs|cjs)$/.test(file));
if(files.length)throw Error('Maintained JavaScript sources must use TypeScript: '+files.join(', '));
console.log('Authored JavaScript source check passed; browser assets are generated from TypeScript.');
