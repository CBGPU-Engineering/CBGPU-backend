import fs from 'node:fs';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
process.chdir(root);
const groups=[['Developer portal','developer']];let out='# Direct npm dependencies\n\nGenerated from committed lockfiles. Versions are installed versions; license fields\nare upstream package metadata, not a legal determination. Transitive dependencies\nare covered by the generated web SBOM where applicable.\n';
for(const [title,root]of groups){const manifest=JSON.parse(fs.readFileSync(root+'/package.json')),lock=JSON.parse(fs.readFileSync(root+'/package-lock.json'));out+='\n## '+title+'\n\n| Package | Version | Scope | Declared license |\n| --- | --- | --- | --- |\n';for(const [scope,items]of Object.entries({runtime:manifest.dependencies||{},development:manifest.devDependencies||{}}))for(const name of Object.keys(items).sort()){const p=lock.packages?.['node_modules/'+name]||{};out+='| `'+name+'` | '+(p.version||'unknown')+' | '+scope+' | '+String(p.license||'Not declared in lockfile').replaceAll('|','\\|')+' |\n'}}
fs.writeFileSync('docs/DEPENDENCIES.md',out);
