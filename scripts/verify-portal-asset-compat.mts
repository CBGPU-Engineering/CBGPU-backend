/** Offline browser check for public asset swaps. All requests are fulfilled locally; no real accounts or APIs. */
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
const args=process.argv.slice(2);
function argument(name:string){const index=args.indexOf(name);assert(index>=0&&args[index+1],`Required argument: ${name}`);return path.resolve(args[index+1]);}
const playwrightModule=argument('--playwright-module'),legacyDeveloper=argument('--legacy-developer-dir'),legacyCareers=argument('--legacy-careers-dir'),output=argument('--out');
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const {chromium}=await import(pathToFileURL(playwrightModule).href);
await fs.mkdir(output,{recursive:true});
const browser=await chromium.launch({headless:true});
const results:any[]=[];
const user={name:'Compatibility preview',email:'preview@example.test'};
const devWhitelist=new Set(['/static/app.js','/static/theme.js','/static/style.css','/static/terminal-client.js','/static/vendor/xterm.js','/static/vendor/addon-fit.js','/static/vendor/xterm.css','/static/assets/logo.png']);
const careerWhitelist=new Set(['/style.css','/app.js','/theme.js','/onboarding.js','/logo.png']);
const read=(file:string)=>fs.readFile(file);
async function darkLogo(page:any,selector:string){
 const result=await page.locator(selector).evaluate((image:HTMLImageElement)=>{
  const canvas=document.createElement('canvas');canvas.width=image.naturalWidth;canvas.height=image.naturalHeight;
  const context=canvas.getContext('2d')!;context.drawImage(image,0,0);const pixels=context.getImageData(0,0,canvas.width,canvas.height).data;
  let transparent=0,opaque=0,black=0;
  for(let i=0;i<pixels.length;i+=4){if(pixels[i+3]===0)transparent++;if(pixels[i+3]===255){opaque++;if(Math.max(pixels[i],pixels[i+1],pixels[i+2])<32)black++;}}
  return{width:image.naturalWidth,height:image.naturalHeight,filter:getComputedStyle(image).filter,content:getComputedStyle(image).content,transparent,opaque,black};
 });
 assert(result.width>0&&result.transparent>0&&result.opaque>0,'Expected loaded transparent logo');
 assert(result.black/result.opaque>.99,'Expected the approved black CBGPU symbol, not the legacy school crest');
 assert.equal(result.filter,'brightness(0) invert(1)');assert.equal(result.content,'normal');return result;
}
try{
 for(const [name,oldHtml,oldScript] of [['new-html-new-js',false,false],['old-html-new-js',true,false],['new-html-old-js',false,true]] as const){
  const context=await browser.newContext({colorScheme:'dark',viewport:{width:390,height:844}});
  const errors:string[]=[],requests:string[]=[];let signed=false;
  await context.route('**/*',async(route:any)=>{
   const url=new URL(route.request().url());requests.push(url.pathname);
   if(url.hostname!=='developers.cbgpu.com')throw Error('Unexpected external request '+url.origin);
   if(url.pathname.startsWith('/api/')){
    const routeName=url.pathname.slice(5);let value:any;
    if(routeName==='session')value={authenticated:signed,csrf_token:signed?'preview-csrf':undefined,user};
    else if(routeName==='login'){assert.equal(route.request().method(),'POST');const body=route.request().postDataJSON();assert.equal(body.email,user.email);assert.equal(body.password,'synthetic-only');signed=true;value={authenticated:true,csrf_token:'preview-csrf',user};}
    else if(routeName==='logout'){assert.equal(route.request().headers()['x-csrf-token'],'preview-csrf');signed=false;value={ok:true};}
    else if(routeName==='overview')value={health:{frontend:true,backend:true,database:true},traffic:{available:false},system:{},deployment:{},usage:{accounts:{dau:3},visitors:{},api:{}},updated_at:'2026-11-24T00:00:00Z'};
    else throw Error('Unexpected synthetic API '+routeName);
    return route.fulfill({json:value});
   }
   let file:string;
   if(url.pathname==='/')file=path.join(oldHtml?legacyDeveloper:path.join(root,'developer/static'),'index.html');
   else{assert(devWhitelist.has(url.pathname),'Outside current Node static whitelist: '+url.pathname);file=path.join(root,'developer',url.pathname.slice(1));if(oldScript&&url.pathname==='/static/app.js')file=path.join(legacyDeveloper,'app.js');}
   const type=file.endsWith('.js')?'application/javascript':file.endsWith('.css')?'text/css':file.endsWith('.png')?'image/png':'text/html';return route.fulfill({body:await read(file),contentType:type});
  });
  const page=await context.newPage();page.on('pageerror',(error:any)=>errors.push(error.message));
  await page.goto('https://developers.cbgpu.com/');await page.waitForSelector('#login-button:not([disabled])');
  const logo=await darkLogo(page,'.login-brand img');
  if(!oldHtml&&!oldScript){await page.emulateMedia({colorScheme:'light'});assert.equal(await page.locator('.login-brand img').evaluate((e:Element)=>getComputedStyle(e).filter),'none');await page.setViewportSize({width:1440,height:900});await page.screenshot({path:path.join(output,'developer-new-html-new-js-light.png')});await page.setViewportSize({width:390,height:844});await page.emulateMedia({colorScheme:'dark'});}
  await page.locator('#login-email').fill(user.email);await page.locator('#password').fill('synthetic-only');await page.locator('#login-button').click();
  await page.waitForSelector('#portal-view:not([hidden])');await page.getByText('Healthy',{exact:true}).first().waitFor();
  if(!oldHtml&&!oldScript)await page.getByText('Active accounts · 1 day',{exact:true}).waitFor();
  assert.equal(await page.locator('#usage-content').count(),oldHtml?0:1);
  await page.screenshot({path:path.join(output,'developer-'+name+'-dark.png'),fullPage:false});
  if(!oldScript){await page.locator('#logout-button').click();await page.waitForSelector('#login-view:not([hidden])');assert.equal(signed,false);assert.equal(await page.locator('#password').inputValue(),'');assert.equal(await page.locator('#login-email').inputValue(),'');assert.equal(await page.locator('#table-result').textContent(),'');}
  assert.deepEqual(errors,[]);assert(!requests.some(p=>p.endsWith('.svg')));results.push({service:'developer',combination:name,logo,errors,requests,verified:oldScript?'New HTML with legacy script startup/sign-in; legacy missing signOut is not invoked.':'Startup, synthetic sign-in, overview, CSRF logout and sensitive-DOM clearing.'});await context.close();
 }
 for(const [name,oldHtml,oldScript] of [['new-html-new-js',false,false],['old-html-new-js',true,false],['new-html-old-js',false,true]] as const){
  const context=await browser.newContext({colorScheme:'dark',viewport:{width:390,height:844}});const errors:string[]=[],requests:string[]=[];
  await context.addInitScript(()=>{Object.defineProperty(navigator,'clipboard',{value:{writeText:async(text:string)=>{if((window as any).clipboardFail)throw Error('synthetic clipboard denial');(window as any).copied=text}}});});
  await context.route('**/*',async(route:any)=>{const url=new URL(route.request().url());requests.push(url.pathname);assert.equal(url.hostname,'careers.cbgpu.com');let file:string;
   if(url.pathname==='/onboarding')file=path.join(oldHtml?legacyCareers:path.join(root,'careers/public'),'onboarding.html');else{assert(careerWhitelist.has(url.pathname),'Outside runtime careers public asset set: '+url.pathname);file=path.join(root,'careers/public',url.pathname.slice(1));if(oldScript&&url.pathname==='/onboarding.js')file=path.join(legacyCareers,'onboarding.js');}
   return route.fulfill({body:await read(file),contentType:file.endsWith('.js')?'application/javascript':file.endsWith('.css')?'text/css':file.endsWith('.png')?'image/png':'text/html'});
  });
  const page=await context.newPage();page.on('pageerror',(error:any)=>errors.push(error.message));await page.goto('https://careers.cbgpu.com/onboarding');
  const logo=await darkLogo(page,'.brand img');await page.screenshot({path:path.join(output,'onboarding-'+name+'-top-dark.png'),fullPage:false});await page.locator('#copy-intro').click();
  const feedback=oldHtml||oldScript?page.locator('#copy-intro'):page.locator('#copy-intro-status');await feedback.filter({hasText:'복사했어요'}).waitFor();assert((await page.evaluate(()=>(window as any).copied)).includes('GitHub ID'));
  await page.evaluate(()=>(window as any).clipboardFail=true);await page.locator('#copy-intro').click();await feedback.filter({hasText:oldHtml||oldScript?'직접 선택':'복사하지 못했어요'}).waitFor();
  if(!oldHtml&&!oldScript){assert.equal(await feedback.getAttribute('role'),'status');assert.equal(await feedback.getAttribute('aria-live'),'polite');}
  await page.screenshot({path:path.join(output,'onboarding-'+name+'-dark.png'),fullPage:false});assert.deepEqual(errors,[]);assert(!requests.some(p=>p.endsWith('.svg')));results.push({service:'careers',combination:name,logo,errors,requests,verified:'Clipboard success and denial feedback; no external posting.'});await context.close();
 }
 await fs.writeFile(path.join(output,'report.json'),JSON.stringify({offline:true,results,limits:['No real authentication/API/DB/terminal/email/deployment action.','Legacy developer script has pre-existing missing signOut implementation; its logout is outside compatibility assertions.']},null,2));console.log(`PASS: ${results.length} old/new asset combinations; dark transparent logos loaded through existing PNG routes; no SVG requests or page errors.`);
}finally{await browser.close();}
