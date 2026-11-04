/** Isolated local verification. Creates a temporary native account without email triggers and removes all fixture data. */
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import http from 'node:http';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {createRequire} from 'node:module';
import {fileURLToPath} from 'node:url';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const require=createRequire(new URL('../developer/package.json',import.meta.url));
const {createClient}=require('@supabase/supabase-js');
const env=Object.fromEntries(fs.readFileSync(path.join(root,'.env'),'utf8').split('\n').filter(l=>l.includes('=')&&!l.startsWith('#')).map(l=>{const i=l.indexOf('=');return[l.slice(0,i),l.slice(i+1)]}));
const admin=createClient('http://127.0.0.1:54321',env.SERVICE_ROLE_KEY,{auth:{persistSession:false,autoRefreshToken:false}});
const account=createClient('http://127.0.0.1:54321',env.ANON_KEY,{auth:{persistSession:false,autoRefreshToken:false}});
const base=process.env.PREPRINTS_TEST_URL||'http://127.0.0.1:3101';
if(!/^http:\/\/127\.0\.0\.1:\d+$/.test(base))throw Error('Verification requires an isolated loopback server.');
const id=crypto.randomUUID(),marker='temporary-preprint-verification-'+crypto.randomUUID(),email=marker+'@example.invalid',password=crypto.randomBytes(24).toString('base64url');
let fixtureCreated=false,paperId,token;
async function sql(text){await new Promise((resolve,reject)=>{const child=spawn('docker',['exec','-i','cbgpu-supabase-db','psql','-X','-U','supabase_admin','-d','postgres','-v','ON_ERROR_STOP=1'],{stdio:['pipe','ignore','pipe']});child.stderr.on('data',()=>{});child.on('error',reject);child.on('exit',code=>code===0?resolve():reject(Error('Fixture SQL failed')));child.stdin.end(text)})}
async function publicPage(site,pathname){return new Promise((resolve,reject)=>{http.get(base+pathname,{headers:{Host:site+'.cbgpu.com','User-Agent':'Googlebot'}},response=>{let text='';response.setEncoding('utf8');response.on('data',chunk=>text+=chunk);response.on('end',()=>resolve({status:response.statusCode,headers:response.headers,text}))}).on('error',reject)})}
async function api(url,init={}){return fetch(base+'/api/arxiv/papers'+url,init)}
function form(pdf=Buffer.from('%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n')){const f=new FormData();f.set('title',marker);f.set('abstract','Temporary integration verification manuscript. This fixture is removed immediately after testing.');f.set('authors',JSON.stringify([{name:'Temporary fixture author',affiliation:'Integration verification'}]));f.set('category','computer-science');f.set('license','all-rights-reserved');f.set('notes','Temporary fixture, removed by verification script.');f.set('pdf',new Blob([pdf],{type:'application/pdf'}),'fixture.pdf');return f}
try{
 const docs=await publicPage('docs','/gpu-notebook');assert.equal(docs.status,200);assert.ok(docs.text.includes('GPU 예약과 Jupyter | CBGPU Docs'));assert.ok(docs.text.includes('https://docs.cbgpu.com/gpu-notebook'));assert.equal(docs.headers['x-robots-tag'],undefined);
 const index=await publicPage('arxiv','/');assert.equal(index.status,200);assert.ok(index.text.includes('CBGPU arXiv'));assert.equal(index.headers['x-robots-tag'],undefined);
 for(const site of ['docs','arxiv']){const robots=await publicPage(site,'/robots.txt');assert.equal(robots.status,200);assert.ok(robots.text.includes(`https://${site}.cbgpu.com/sitemap.xml`));const sitemap=await publicPage(site,'/sitemap.xml');assert.equal(sitemap.status,200);assert.ok(sitemap.text.includes(`https://${site}.cbgpu.com`))}
 assert.equal((await api('',{method:'POST'})).status,401);
 for(const query of ['?category=invalid','?page=0','?page=1.5'])assert.equal((await api(query)).status,400);
 await sql(`BEGIN; SET LOCAL session_replication_role=replica;
 INSERT INTO auth.users(instance_id,id,aud,role,email,encrypted_password,email_confirmed_at,confirmation_token,recovery_token,email_change_token_new,email_change,raw_app_meta_data,raw_user_meta_data,created_at,updated_at) VALUES('00000000-0000-0000-0000-000000000000','${id}','authenticated','authenticated','${email}',extensions.crypt('${password}',extensions.gen_salt('bf')),now(),'','','','','{"provider":"email","providers":["email"]}','{"full_name":"Temporary preprint verification"}',now(),now());
 INSERT INTO auth.identities(provider_id,user_id,identity_data,provider,created_at,updated_at) VALUES('${id}','${id}','{"sub":"${id}","email":"${email}","email_verified":true}','email',now(),now());
 INSERT INTO public.profiles(id,full_name,student_id,student_id_school_year,is_admin,is_approved) VALUES('${id}','Temporary preprint verification','3398',public.cbgpu_school_year(now()),false,true); COMMIT;`);fixtureCreated=true;
 const login=await account.auth.signInWithPassword({email,password});if(login.error)throw Error('Temporary account sign-in failed');token=login.data.session.access_token;
 const headers={Authorization:'Bearer '+token};
 let response=await api('',{method:'POST',headers,body:form(Buffer.from('not a real PDF'))});assert.equal(response.status,400);
 const short=form();short.set('title','a');response=await api('',{method:'POST',headers,body:short});assert.equal(response.status,400);
 response=await api('',{method:'POST',headers,body:form()});const result=await response.json();assert.equal(response.status,201,'Authenticated publication failed: '+(result.message||response.status));paperId=result.paper.id;assert.ok(paperId);assert.equal(result.paper.current_version,1);
 response=await api('?q='+encodeURIComponent(marker)+'&category=computer-science&page=1');const listing=await response.json();assert.equal(response.status,200);assert.ok(listing.papers.some(p=>p.id===paperId));assert.equal(listing.total,1);
 response=await api('/'+paperId);const detail=await response.json();assert.equal(response.status,200);assert.equal(detail.paper.title,marker);assert.equal(detail.versions.length,1);assert.equal(detail.versions[0].version,1);
 const page=await publicPage('arxiv','/'+paperId);assert.equal(page.status,200);assert.ok(page.text.includes('name="citation_title"'));assert.ok(page.text.includes(marker));assert.ok(page.text.includes('https://arxiv.cbgpu.com/'+paperId));
 response=await api('/'+paperId+'/pdf?version=1');assert.equal(response.status,200);assert.match(response.headers.get('content-type'),/application\/pdf/);assert.match(response.headers.get('content-disposition'),/^inline/);assert.equal(Buffer.from(await response.arrayBuffer()).subarray(0,5).toString(),'%PDF-');
 response=await api('/'+paperId+'/pdf?version=1&download=1');assert.equal(response.status,200);assert.match(response.headers.get('content-disposition'),/^attachment/);
 assert.equal((await api('/'+paperId+'/pdf?version=0')).status,400);assert.equal((await api('/'+paperId+'/pdf?version=99')).status,404);
 console.log('Preprint verification passed: anonymous/validation guards, host routes/metadata/robots/sitemaps, native account sign-in, invalid PDF/metadata rejection, publication, search/category, detail/version, inline/download PDF.');
} finally {
 const {data:papers,error:queryError}=await admin.from('preprints').select('id').eq('owner_id',id);if(queryError)throw Error('Fixture cleanup lookup failed');
 for(const paper of papers||[]){const {data:versions,error}=await admin.from('preprint_versions').select('pdf_path').eq('paper_id',paper.id);if(error)throw Error('Fixture version lookup failed');if(versions?.length){const removed=await admin.storage.from('preprints').remove(versions.map(v=>v.pdf_path));if(removed.error)throw Error('Fixture PDF cleanup failed')}const removed=await admin.from('preprints').delete().eq('id',paper.id);if(removed.error)throw Error('Fixture paper cleanup failed')}
 if(fixtureCreated){const removed=await admin.auth.admin.deleteUser(id);if(removed.error)throw Error('Fixture user cleanup failed')}
 await account.auth.signOut();
 console.log('Temporary preprint, stored PDF and native account cleanup completed.');
}
