const fs=require('fs'),crypto=require('crypto');
const {DatabaseSync}=require('node:sqlite');
const puppeteer=require(require('path').resolve(__dirname,'../../FrontEnd/cbgpu/node_modules/puppeteer-core'));
const root=require('path').resolve(__dirname,'..');
const settings=JSON.parse(fs.readFileSync(root+'/private/developer-portal.json'));
const token=crypto.randomBytes(32).toString('hex'),csrf=crypto.randomBytes(32).toString('hex'),hash=crypto.createHmac('sha256',settings.sessionSecret).update(token).digest('hex');
const db=new DatabaseSync(root+'/private/developer.sqlite');
db.prepare('INSERT INTO sessions(token_hash,csrf,expires) VALUES(?,?,?)').run(hash,csrf,Date.now()+600000);
const base='http://127.0.0.1:3400';const cookie='__Host-cbgpu-developer='+token;
async function api(route,body){const r=await fetch(base+'/api'+route,{method:body?'POST':'GET',headers:{Cookie:cookie,Origin:base,'X-CSRF-Token':csrf,'Content-Type':'application/json'},...(body?{body:JSON.stringify(body)}:{})});const data=await r.json();if(!r.ok)throw Error(route+': '+data.error);return data}
(async()=>{let browser;try{
 const unauth=await fetch(base+'/api/operations');if(unauth.status!==401)throw Error('Operations auth failed');
 const unauthLog=await fetch(base+'/api/logs?source=database');if(unauthLog.status!==401)throw Error('Logs auth failed');
 await api('/sql',{query:'CREATE TABLE developer_workspace.portal_ui_check (id integer PRIMARY KEY, note text)'});
 await api('/sql',{query:"INSERT INTO developer_workspace.portal_ui_check VALUES (1, 'alpha'), (2, 'beta <script>unsafe</script>') RETURNING *"});
 const filtered=await api('/table?schema=developer_workspace&name=portal_ui_check&search=beta&sort=id&direction=desc');if(filtered.total!==1||filtered.rows[0].id!==2)throw Error('Filtered rows mismatch');
 const bad=await fetch(base+'/api/table?schema=developer_workspace&name=portal_ui_check&offset=NaN',{headers:{Cookie:cookie}});if(bad.status!==400)throw Error('Pagination validation failed');
 const disabled=await fetch(base+'/api/deploy',{method:'POST',headers:{Cookie:cookie,Origin:base,'X-CSRF-Token':csrf,'Content-Type':'application/json'},body:JSON.stringify({action:'deploy'})});if(disabled.status!==405)throw Error('Deploy endpoint still enabled');
 const ops=await api('/operations');if(!ops.containers.length||!ops.backups.length)throw Error('Operations missing real records');console.log('Operations:',ops.containers.length,'containers;',ops.backups.length,'backups');
 for(const source of ['frontend','auth','functions','database']){const x=await api('/logs?source='+source);console.log('Log source:',source,!!x.updated_at)}
 browser=await puppeteer.launch({executablePath:'/opt/brave.com/brave/brave',headless:true,args:['--no-sandbox','--disable-dev-shm-usage']});
 for(const [name,width,height] of [['desktop',1440,1000],['mobile',390,844]]){
  const page=await browser.newPage();const errors=[];page.on('response',r=>{if(r.status()>=400)console.log('HTTP',r.status(),r.url())});page.on('pageerror',e=>errors.push(e.message));page.on('console',m=>{if(m.type()==='error')errors.push(m.text())});page.on('dialog',d=>d.accept());await page.setViewport({width,height});
  await page.setCookie({name:'__Host-cbgpu-developer',value:token,url:base,secure:true,httpOnly:true,sameSite:'Strict'});
  await page.goto(base,{waitUntil:'networkidle0'});await page.waitForSelector('.health-card');
  for(const theme of ['light','dark']){await page.select('.header-actions [data-theme-choice]',theme);await page.screenshot({path:'/tmp/cbgpu-developer-new-'+name+'-'+theme+'.png',fullPage:true});if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error(name+' '+theme+' overview overflows')}
  await page.reload({waitUntil:'networkidle0'});if(await page.$eval('html',e=>e.dataset.theme)!=='dark')throw Error('Theme not persisted');
  await page.click('[data-panel="database"]');await page.waitForSelector('.table-link');await page.type('#table-search','portal_ui_check');await page.click('.table-link');await page.waitForFunction(()=>document.querySelector('#table-count').textContent.includes('2'));
  await page.type('#row-search','beta');await page.click('#row-filter-form button[type="submit"]');await page.waitForFunction(()=>document.querySelector('#table-count').textContent.includes('of 1'));
  if(await page.$eval('#table-result',e=>!!e.querySelector('script')))throw Error('Unsafe row rendering');
  await page.click('#table-schema');if(await page.$eval('#schema-result',e=>e.hidden))throw Error('Schema hidden');await page.screenshot({path:'/tmp/cbgpu-developer-new-table-'+name+'.png',fullPage:true});
  await page.click('[data-panel="sql"]');await page.select('#sql-snippet','size');await page.click('#sql-run');await page.waitForSelector('#sql-result td');if(await page.$eval('#sql-export',e=>e.disabled))throw Error('SQL export disabled');
  await page.click('[data-panel="deployments"]');if(await page.$('[data-deploy-action]'))throw Error('Deployment action visible');await page.waitForFunction(()=>document.querySelector('#release-details').textContent.includes('cbgpu.com'));await page.screenshot({path:'/tmp/cbgpu-developer-new-deployments-'+name+'.png',fullPage:true});
  await page.click('[data-panel="operations"]');await page.waitForSelector('.service-row');await page.waitForFunction(()=>document.querySelector('#logs-message').textContent.includes('Last 150'));await page.screenshot({path:'/tmp/cbgpu-developer-new-services-'+name+'.png',fullPage:true});
  if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error(name+' services overflows');
  await page.click('[data-panel="terminal"]');await page.evaluate(()=>{window.__ptyOutput='';const Native=window.WebSocket;window.WebSocket=class extends Native{constructor(...args){super(...args);this.addEventListener('message',e=>{const m=JSON.parse(e.data);if(m.type==='output')window.__ptyOutput+=m.data})}}});await page.click('#terminal-connect');await page.waitForFunction(()=>document.querySelector('#terminal-status').textContent.includes('Connected'));await page.type('.xterm-helper-textarea',"ls -d backend\n");await page.waitForFunction(()=>window.__ptyOutput.includes('backend'));await page.screenshot({path:'/tmp/cbgpu-developer-new-terminal-'+name+'.png',fullPage:true});
  if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth))throw Error(name+' terminal overflows');
  await page.click('#terminal-disconnect');if(await page.$('.project-switch'))throw Error('Project banner still visible');if(errors.length)throw Error(name+' browser errors: '+errors.join('; '));console.log(name,'passed: real APIs, theme persistence, tables/search/schema, SQL, releases, services/logs, responsive layout');await page.close();
 }
}finally{await api('/sql',{query:'DROP TABLE IF EXISTS developer_workspace.portal_ui_check'}).catch(()=>{});db.prepare('DELETE FROM sessions WHERE token_hash=?').run(hash);db.close();await browser?.close()}})().catch(e=>{console.error(e);process.exit(1)});
