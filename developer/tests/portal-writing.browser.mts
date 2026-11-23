/** Actual static UIs; every request is fulfilled locally. No service login or DB/mail writes.
 * Optional Playwright package: CBGPU_BROWSER_TEST_TOOLS=/path/to/package.json containing playwright.
 * CBGPU_COPY_BASELINE=<git revision> captures/asserts previous copy with the same fixture actions.
 */
import assert from 'node:assert/strict';
import {readFile,mkdir,writeFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {createRequire} from 'node:module';
import {execFileSync} from 'node:child_process';
import {build} from 'esbuild';
const repo=fileURLToPath(new URL('../../',import.meta.url));
const browserTools=createRequire(process.env.CBGPU_BROWSER_TEST_TOOLS||import.meta.url);
const {chromium}=browserTools('playwright');
const baseline=process.env.CBGPU_COPY_BASELINE;
if(baseline)assert.match(baseline,/^[a-f0-9]{7,40}$/);
const output=path.resolve(process.env.CBGPU_COPY_PROOF_DIR||'/tmp/cbgpu-backend-writing-'+(baseline?'before':'after'));
await mkdir(output,{recursive:true});
const source=async(file:string)=>baseline?execFileSync('git',['show',baseline+':'+file],{cwd:repo,encoding:'utf8'}):readFile(path.join(repo,file),'utf8');
const assets:Record<string,string>={};
for(const file of ['careers/public/app.ts','careers/public/onboarding.ts','careers/public/theme.ts','developer/static/app.ts','developer/static/theme.ts']){
 const script=await build({stdin:{contents:await source(file),resolveDir:path.dirname(path.join(repo,file)),loader:'ts'},bundle:true,write:false,platform:'browser',format:'iife',target:'es2020'});
 assets[file.replace(/\.ts$/,'.js')]=script.outputFiles[0].text;
}
const roles=JSON.parse(await source('careers/roles.json'));
const origin='http://127.0.0.1:15995';
const browser=await chromium.launch({headless:true,...(process.env.CBGPU_BROWSER_EXECUTABLE?{executablePath:process.env.CBGPU_BROWSER_EXECUTABLE}:{}),args:['--no-sandbox','--disable-dev-shm-usage']});
let external=0,unexpected=0,simulatedApplications=0,simulatedUpdates=0;const results:unknown[]=[];
try{
 for(const width of[390,1280])for(const dark of[false,true])for(const site of['developer','careers']){
  const context=await browser.newContext({viewport:{width,height:950},colorScheme:dark?'dark':'light',reducedMotion:'reduce',serviceWorkers:'block'}),page=await context.newPage(),errors:string[]=[];
  page.on('pageerror',(error:Error)=>errors.push(error.message));
  let applications=0,overviewFailure=false,tablePopulated=false;
  await context.addInitScript(()=>{Object.defineProperty(navigator,'clipboard',{value:{writeText:async(text:string)=>{if((window as any).__failClipboard)throw Error('Synthetic clipboard denial');(window as any).__copied=text}}})});
  await context.route('**/*',async(route:any)=>{
   const request=route.request(),url=new URL(request.url());
   if(url.origin!==origin){external++;return route.abort();}
   const json=(body:unknown,status=200)=>route.fulfill({status,json:body});
   if(url.pathname.startsWith('/api/')){
    if(site==='careers'){
     if(url.pathname==='/api/roles')return json(roles);
     if(url.pathname==='/api/form')return json({token:'synthetic-form-only'});
     if(url.pathname==='/api/applications'&&request.method()==='POST'){
      const body=request.postDataJSON();assert.equal(body.role,'frontend');assert.equal(body.email,'synthetic@example.invalid');assert.equal(body.consent,true);assert.equal(body.token,'synthetic-form-only');
      applications++;simulatedApplications++;return applications===1?json({error:'입력 내용을 확인해 주세요.'},422):json({reference:'SYNTHETIC-REFERENCE'});
     }
    }else if(url.pathname==='/api/table/update'&&request.method()==='POST'){
     assert.deepEqual(request.postDataJSON(),{schema:'public',table:'synthetic_table',primary_key:{id:1},values:{note:'Synthetic edit'}});simulatedUpdates++;return route.fulfill({status:200,contentType:'text/html',body:'synthetic invalid JSON acknowledgement'});
    }else if(request.method()==='GET'){
     if(url.pathname==='/api/session')return json({authenticated:true,csrf_token:'synthetic-local-only',user:{name:'Synthetic operator',email:'operator@example.invalid'}});
     if(url.pathname==='/api/overview')return overviewFailure?json({error:'Synthetic read unavailable'},503):json({});
     if(url.pathname==='/api/account')return json({user:{name:'Synthetic operator',email:'operator@example.invalid'},developers:[],recovery_kit_available:false});
     if(url.pathname==='/api/tables')return json({tables:[{schema:'public',name:'synthetic_table',columns:[{name:'id',type:'integer',primary_key:true},{name:'note',type:'text'}]}]});
     if(url.pathname==='/api/table')return json({rows:tablePopulated?[{id:1,note:'Original synthetic value'}]:[],columns:[],total:tablePopulated?1:0});
     if(url.pathname==='/api/audit')return json({events:[]});
     if(url.pathname==='/api/deployments')return json({jobs:[]});
    }
    unexpected++;return route.abort();
   }
   if(url.pathname.endsWith('.js')){
    const file=site==='developer'?'developer'+url.pathname:'careers/public'+url.pathname;
    return route.fulfill({contentType:'application/javascript',body:assets[file]||''});
   }
   if(url.pathname.endsWith('.css'))return route.fulfill({contentType:'text/css',body:await source(site==='developer'?'developer/static/style.css':'careers/public/style.css')});
   if(/\.(png|svg|ico)$/.test(url.pathname)){const asset=site==='developer'?'developer'+url.pathname:'careers/public'+url.pathname;try{return route.fulfill({contentType:url.pathname.endsWith('.svg')?'image/svg+xml':'image/png',body:await readFile(path.join(repo,asset))})}catch{return route.fulfill({status:204,body:''})}}
   return route.fulfill({contentType:'text/html',body:await source(site==='developer'?'developer/static/index.html':url.pathname==='/onboarding'?'careers/public/onboarding.html':'careers/public/index.html')});
  });
  const shot=async(state:string)=>{await page.screenshot({path:path.join(output,`${site}-${state}-${width}-${dark?'dark':'light'}.png`),fullPage:true});};
  try{
   await page.goto(origin);
   if(site==='developer'){
    await page.locator('#portal-view').waitFor({state:'visible'});await page.locator('[data-panel="security"]').click();
    await page.getByText(baseline?'Account security is up to date.':'Account security details loaded.',{exact:true}).waitFor();await shot('account');
    await page.locator('[data-panel="database"]').click();await page.getByText(baseline?'This table has no rows.':'No rows match the current view.',{exact:true}).waitFor();await shot('table-empty');
    await page.locator('#row-search').fill('synthetic-nonmatching');await page.locator('#row-filter-form').evaluate((form:HTMLFormElement)=>form.requestSubmit());
    await page.getByText(baseline?'This table has no rows.':'No rows match the current view.',{exact:true}).waitFor();assert.equal(await page.locator('#row-search').inputValue(),'synthetic-nonmatching');
    tablePopulated=true;await page.locator('#row-filter-clear').click();await page.getByRole('button',{name:'Edit row 1',exact:true}).click();await page.locator('#edit-field-1').fill('Synthetic edit');page.once('dialog',(dialog:any)=>dialog.accept());await page.locator('#row-editor-save').click();
    await page.getByText(baseline?'Row updated.':'Update request returned. Check the refreshed row to verify the result.',{exact:true}).waitFor();await page.getByRole('cell',{name:'Original synthetic value',exact:true}).waitFor();await shot('ambiguous-update');
    await page.locator('[data-panel="audit"]').click();await page.getByText(baseline?'No audit events have been recorded yet.':'No audit events were returned.',{exact:true}).waitFor();await shot('audit-empty');
    await page.locator('[data-panel="deployments"]').click();await page.getByText(baseline?'No deployment jobs yet. Build a release to get started.':'No deployment jobs were returned.',{exact:true}).waitFor();await shot('deployments-empty');
    overviewFailure=true;await page.locator('[data-panel="overview"]').click();await page.getByText(baseline?'Refresh failed.':'Could not refresh the overview. Select Refresh to try again.',{exact:true}).waitFor();await shot('overview-error');
    await page.locator('[data-panel="terminal"]').click();await page.getByText(baseline?'Use this computer’s shell directly, including interactive programs.':'Open an interactive shell on the CBGPU server.',{exact:true}).waitFor();await shot('terminal-description');
   }else{
    await page.locator('#roles .role-row').first().waitFor();assert.equal(await page.locator('#roles .role-row').count(),roles.length);
    await page.getByText(baseline?'Engineering · 교내 프로젝트':'개발 · 교내 프로젝트',{exact:true}).first().waitFor();await shot('roles');
    await page.locator('a[href="/roles/frontend"]').click();await page.locator('.detail .eyebrow').waitFor();assert.match(await page.locator('.detail .eyebrow').innerText(),baseline?/Engineering/:/개발/);await shot('role-detail');
    await page.locator('a[href="/apply/frontend"]').click();await page.locator('#submit:not([disabled])').waitFor();
    await page.getByText(baseline?/임시 저장은 지원하지 않아요\./:/페이지를 떠나면 입력 내용이 사라져요\./).waitFor();await shot('application-form');
    await page.locator('[name="name"]').fill('합성 지원자');await page.locator('[name="email"]').fill('synthetic@example.invalid');await page.locator('[name="motivation"]').fill('실제 외부 전송 없이 문구를 확인하는 합성 지원 내용이에요.');await page.locator('[name="consent"]').check();await page.locator('#submit').click();
    await page.getByText(baseline?/내용은 그대로 있으니 잠시 후 다시 시도해주세요/:/입력 내용은 이 화면에 남아 있어요\. 안내를 확인한 뒤 다시 제출해 주세요/).waitFor();assert.equal(await page.locator('[name="name"]').inputValue(),'합성 지원자');assert.equal(applications,1);await shot('application-error');
    await page.locator('#submit').click();await page.getByText('접수 번호: SYNTHETIC-REFERENCE',{exact:true}).waitFor();await page.getByText(baseline?/검토 후 적어주신 이메일로 연락드릴게요/:/지원서 검토와 연락에 적어주신 이메일을 사용해요/).waitFor();assert.equal(applications,2);await shot('application-received');
    await page.goto(origin+'/onboarding');await page.locator('#copy-intro').click();await page.getByText(baseline?'양식을 복사했어요. Slack에 붙여넣고 내용을 채워주세요.':'양식을 복사했어요. Slack에 붙여 넣고 내용을 채워 주세요.',{exact:true}).waitFor();assert.match(await page.evaluate(()=>(window as any).__copied),/GitHub/);await page.getByText('체크 상태는 저장하지 않아요.',{exact:true}).waitFor();await shot('onboarding-copy');await page.evaluate(()=>{(window as any).__failClipboard=true});await page.locator('#copy-intro').click();await page.getByText(baseline?'복사하지 못했어요. 위 양식을 직접 선택해 복사해주세요.':'복사하지 못했어요. 위 양식을 직접 선택해 복사해 주세요.',{exact:true}).waitFor();await shot('onboarding-copy-error');
   }
   assert.deepEqual(errors,[]);results.push({site,width,dark,syntheticOnly:true});
  }finally{await context.close();}
 }
 assert.equal(external,0);assert.equal(unexpected,0);assert.equal(simulatedApplications,8);assert.equal(simulatedUpdates,4);
 await writeFile(path.join(output,'manifest.json'),JSON.stringify({baseline:baseline||null,results,external,unexpected,simulatedApplications,simulatedUpdates},null,2));console.log(JSON.stringify({cases:results.length,external,unexpected,simulatedApplications,simulatedUpdates,output}));
}finally{await browser.close();}
