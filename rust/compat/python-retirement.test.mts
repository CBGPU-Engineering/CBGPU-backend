import test from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {readFileSync,existsSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
const root=fileURLToPath(new URL('../../',import.meta.url));
test('maintained Python implementation is retired, not hidden in language statistics',()=>{
 const files=execFileSync('git',['ls-files','-z'],{cwd:root,encoding:'utf8'}).split('\0').filter(f=>f&&existsSync(path.join(root,f)));
 assert.deepEqual(files.filter(f=>/\.(?:py|pyi|pyc)$/i.test(f)),[]);
 const attrs=existsSync(new URL('../../.gitattributes',import.meta.url))?readFileSync(new URL('../../.gitattributes',import.meta.url),'utf8'):'';
 assert.doesNotMatch(attrs,/linguist-(?:language|vendored|generated).*Python|\*\.py.*linguist-/i);
});
test('live legacy release is not reconstructed from removed source',()=>{
 const workflow=readFileSync(new URL('../../.github/workflows/ci.yml',import.meta.url),'utf8');
 assert.doesNotMatch(workflow,/python3\s+-m\s+(?:unittest|compileall)/);
 assert.match(workflow,/--test model_assets --test native_resources/);
 assert.match(workflow,/--test public_start/);
 const bridge=readFileSync(new URL('../../gpu/server.mts',import.meta.url),'utf8');
 assert.doesNotMatch(bridge,/start-public\.py|['"]python3['"]/);
 assert.match(bridge,/CBGPU_GPU_PUBLIC_STARTER/);
 const unit=readFileSync(new URL('../../ai/cbgpu-ai.service',import.meta.url),'utf8');
 assert.match(unit,/ConditionPathExists=.*ai-native-reviewed/);
 assert.doesNotMatch(unit,/python|server\.py/);
});
