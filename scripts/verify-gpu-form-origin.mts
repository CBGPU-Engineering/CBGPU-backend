/** Optional real-browser regression: CBGPU_PLAYWRIGHT_MODULE points to installed Playwright. */
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createServer} from 'node:http';
const modulePath=process.env.CBGPU_PLAYWRIGHT_MODULE;
assert(modulePath,'Set CBGPU_PLAYWRIGHT_MODULE to the installed playwright module');
const {chromium}=await import(modulePath);
const source=await readFile(new URL('../rust/crates/cbgpu-gpu-gateway/src/lib.rs',import.meta.url),'utf8');
const policy=source.match(/h\.insert\("referrer-policy", "([^"]+)"\.parse\(\)\.unwrap\(\)\);/)?.[1];
assert.equal(policy,'strict-origin');
let seen:{origin:string|undefined;referer:string|undefined}|undefined;
const server=createServer((req,res)=>{
 if(req.method==='POST'){seen={origin:req.headers.origin,referer:req.headers.referer};res.end('saved');return;}
 res.setHeader('Referrer-Policy',policy!);res.setHeader('Content-Type','text/html');
 res.end('<form method="post" action="/1/login"><input name="password" value="synthetic"><button>Sign in</button></form>');
});
await new Promise<void>(resolve=>server.listen(0,'127.0.0.1',resolve));
const address=server.address();assert(address&&typeof address!=='string');const origin=`http://127.0.0.1:${address.port}`;
const browser=await chromium.launch({headless:true,args:['--no-sandbox']});
try{const page=await browser.newPage();await page.goto(origin+'/1/login?private=never-send');await Promise.all([page.waitForURL(origin+'/1/login'),page.getByRole('button',{name:'Sign in'}).click()]);assert.equal(seen?.origin,origin);assert.equal(seen?.referer,origin+'/');console.log('PASS: Chromium same-origin form preserves Origin and sends only origin Referer');}
finally{await browser.close();await new Promise<void>(resolve=>server.close(()=>resolve()));}
