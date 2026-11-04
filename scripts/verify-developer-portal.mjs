import fs from 'node:fs';
const base='http://127.0.0.1:3400';let cookie='',csrf='';
async function request(route,body,headers={}){const r=await fetch(base+route,{method:body===undefined?'GET':'POST',headers:{...(body!==undefined?{'Content-Type':'application/json','Origin':base,'X-CSRF-Token':csrf}:{}),Cookie:cookie,...headers},...(body!==undefined?{body:JSON.stringify(body)}:{})});return{r,data:await r.json()}}
let {r}=await request('/api/overview');if(r.status!==401)throw Error('Unauthenticated dashboard exposed');
({r}=await request('/api/login',{password:'incorrect-test-value'}));if(r.status!==401)throw Error('Incorrect password accepted');
const file='/home/cbgpu/CBGPU-live/backend/private/developer-test-password';let result;
try{result=await request('/api/login',{password:fs.readFileSync(file,'utf8').trim()})}finally{fs.unlinkSync(file)}
if(!result.r.ok)throw Error('Login failed');csrf=result.data.csrf_token;const setCookie=result.r.headers.get('set-cookie');if(!/HttpOnly/.test(setCookie)||!/Secure/.test(setCookie)||!/SameSite=Strict/.test(setCookie))throw Error('Cookie security flags missing');cookie=setCookie.split(';')[0];
let bad=await request('/api/sql',{query:'select 1'},{'X-CSRF-Token':''});if(bad.r.status!==403)throw Error('CSRF guard failed');
bad=await request('/api/sql',{query:'select 1'},{Origin:'https://untrusted.example'});if(bad.r.status!==403)throw Error('Origin guard failed');
result=await request('/api/sql',{query:'select current_user,rolsuper,rolcreaterole,rolcreatedb from pg_roles where rolname=current_user'});if(!result.r.ok||result.data.rows[0].current_user!=='cbgpu_developer'||result.data.rows[0].rolsuper||result.data.rows[0].rolcreaterole||result.data.rows[0].rolcreatedb)throw Error('Database privilege isolation failed');
bad=await request('/api/sql',{query:'select * from auth.users limit 1'});if(bad.r.ok)throw Error('Protected Auth tables exposed to query role');
for(const query of ['CREATE TABLE developer_workspace.portal_test (id integer PRIMARY KEY,value text)','INSERT INTO developer_workspace.portal_test VALUES (1,\'before\') RETURNING *']){result=await request('/api/sql',{query});if(!result.r.ok)throw Error('Workspace SQL failed: '+result.data.error)}
result=await request('/api/tables');if(!result.r.ok||!result.data.tables.length)throw Error('Table catalogue failed');
result=await request('/api/table/update',{schema:'developer_workspace',table:'portal_test',primary_key:{id:1},values:{value:'<script>test</script>'}});if(!result.r.ok)throw Error('Primary-key row update failed: '+result.data.error);
result=await request('/api/table?schema=developer_workspace&name=portal_test');if(!result.r.ok||result.data.rows[0].value!=='<script>test</script>')throw Error('Table read failed');
result=await request('/api/sql',{query:'DROP TABLE developer_workspace.portal_test'});if(!result.r.ok)throw Error('Workspace cleanup failed');
result=await request('/api/overview');if(!result.r.ok||!result.data.health.database)throw Error('Health overview failed');
result=await request('/api/logout',{});if(!result.r.ok)throw Error('Logout failed');result=await request('/api/overview');if(result.r.status!==401)throw Error('Session revocation failed');
console.log('Developer portal authentication, CSRF, permissions, table CRUD, SQL and logout checks passed.');
