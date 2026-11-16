import {build} from 'esbuild';
import {copyFile,mkdir} from 'node:fs/promises';
for(const folder of ['careers/public','developer/static']) {
 const names=folder.startsWith('careers')?['app','onboarding','theme']:['app','equipment-signin','terminal-client','theme'];
 for(const name of names)await build({entryPoints:[`${folder}/${name}.ts`],outfile:`${folder}/${name}.js`,bundle:true,format:'iife',platform:'browser',target:'es2020',legalComments:'linked'});
}
await mkdir('developer/static/vendor',{recursive:true});
await copyFile('developer/node_modules/@xterm/xterm/lib/xterm.js','developer/static/vendor/xterm.js');
await copyFile('developer/node_modules/@xterm/addon-fit/lib/addon-fit.js','developer/static/vendor/addon-fit.js');

await build({entryPoints:['deploy/edge-worker.ts'],outfile:'deploy/edge-worker.js',bundle:true,format:'esm',platform:'browser',target:'es2022'});
