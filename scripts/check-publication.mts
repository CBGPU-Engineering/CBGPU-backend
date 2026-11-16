import {execFileSync}from'node:child_process';import fs from'node:fs';
const files=execFileSync('git',['ls-files','-z'],{encoding:'utf8'}).split('\0').filter(Boolean),problems=[];
for(const file of files){
 if(/(?:^|\/)(?:private|backups|node_modules|\.venv|\.dart_tool)(?:\/|$)|\.(?:jks|keystore|p12|apk|bundle)$|(?:^|\/)key\.properties$/.test(file))problems.push('Private/generated file tracked: '+file);
 if(/(?:^|\/)\.env(?:\.|$)/.test(file)&&!file.endsWith('.example'))problems.push('Environment file tracked: '+file);
 if(!fs.existsSync(file)||!fs.statSync(file).isFile()||fs.statSync(file).size>2000000)continue;
 const text=fs.readFileSync(file,'utf8');
 if(/-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/.test(text))problems.push('Private key found: '+file);
 if(/(?:ghp|gho|ghu|ghs|github_pat)_[A-Za-z0-9_]{30,}/.test(text))problems.push('GitHub token found: '+file);
}
for(const file of ['LICENSE','NOTICE','CONTRIBUTING.md','SECURITY.md','CODE_OF_CONDUCT.md','CHANGELOG.md','THIRD_PARTY_NOTICES.md'])if(!fs.existsSync(file))problems.push('Missing '+file);
if(problems.length){console.error(problems.join('\n'));process.exit(1)}console.log('Publication check passed: no tracked runtime state, APKs, private keys or GitHub tokens. This complements provider secret scanning; it is not a complete secret detector.');
