/** Temporary local CBGPU identities only. This check never calls a school mutation. */
const fs=require('fs'),http=require('http'),https=require('https'),crypto=require('crypto'),assert=require('assert/strict'),{execFileSync}=require('child_process'),{createClient}=require('../../FrontEnd/cbgpu/node_modules/@supabase/supabase-js');
const root='/home/cbgpu/CBGPU-live',env=Object.fromEntries(fs.readFileSync(root+'/backend/.env','utf8').split('\n').filter(l=>l.includes('=')&&!l.startsWith('#')).map(l=>{const i=l.indexOf('=');return [l.slice(0,i),l.slice(i+1)]})),port=process.env.CBGPU_TEST_FRONTEND_PORT||'3002';
const admin=createClient('http://127.0.0.1:54321',env.SERVICE_ROLE_KEY,{auth:{persistSession:false,autoRefreshToken:false}}),client=createClient('http://127.0.0.1:54321',env.ANON_KEY,{auth:{persistSession:false,autoRefreshToken:false}});
const id=crypto.randomUUID(),email='study-contract-'+id+'@example.invalid',password=crypto.randomBytes(24).toString('base64url');let created=false;
const sql=q=>execFileSync('docker',['exec','cbgpu-supabase-db','psql','-X','-U','supabase_admin','-d','postgres','-At','-v','ON_ERROR_STOP=1','-c',q],{encoding:'utf8'});
async function api(path,token,body,origin='https://app.cbgpu.com'){return new Promise((resolve,reject)=>{const req=(process.env.CBGPU_TEST_PUBLIC==='1'?https:http).request({hostname:process.env.CBGPU_TEST_PUBLIC==='1'?'app.cbgpu.com':'127.0.0.1',port:process.env.CBGPU_TEST_PUBLIC==='1'?443:port,path,method:body===undefined?'GET':'POST',headers:{Host:'app.cbgpu.com',Origin:origin,'Content-Type':'application/json',...(token?{Authorization:'Bearer '+token}:{})}},res=>{let value='';res.on('data',c=>value+=c);res.on('end',()=>{try{resolve({status:res.statusCode,body:JSON.parse(value)})}catch(e){reject(e)}})});req.on('error',reject);req.end(body===undefined?undefined:JSON.stringify(body))})}

(async()=>{try{
 assert.equal((await api('/api/study/link','',{action:'start'})).status,401);
 sql(`BEGIN;SET LOCAL session_replication_role=replica;INSERT INTO auth.users(instance_id,id,aud,role,email,encrypted_password,email_confirmed_at,confirmation_token,recovery_token,email_change_token_new,email_change,raw_app_meta_data,raw_user_meta_data,created_at,updated_at) VALUES('00000000-0000-0000-0000-000000000000','${id}','authenticated','authenticated','${email}',extensions.crypt('${password}',extensions.gen_salt('bf')),now(),'','','','','{"provider":"email","providers":["email"]}','{"full_name":"Study integration fixture"}',now(),now());INSERT INTO auth.identities(provider_id,user_id,identity_data,provider,created_at,updated_at) VALUES('${id}','${id}','{"sub":"${id}","email":"${email}","email_verified":true}','email',now(),now());INSERT INTO public.profiles(id,full_name,student_id,student_id_school_year,is_admin,is_approved) VALUES('${id}','Study integration fixture','3395',public.cbgpu_school_year(now()),false,true);COMMIT;`);created=true;
 const {data,error}=await client.auth.signInWithPassword({email,password});assert.ifError(error);const token=data.session.access_token;
 const noSchool=await api('/api/study/student',token);assert.equal(noSchool.status,401);assert.match(noSchool.body.message,/EduSet/);
 assert.equal(noSchool.body.code,'school_connection_required');
 const privateRead=await client.from('eduset_student_connections').select('*');assert.ok(privateRead.error,'student connections must not be readable by authenticated clients');
 const secretLines=fs.readFileSync(root+'/backend/private/frontend-runtime.env','utf8').split('\n');const secretLine=secretLines.find(l=>l.startsWith('EDUSET_TOKEN_SECRET='));assert.ok(secretLine);const secret=secretLine.slice(secretLine.indexOf('=')+1).replace(/^['"]|['"]$/g,'');
 const seal=value=>{const iv=crypto.randomBytes(12),cipher=crypto.createCipheriv('aes-256-gcm',crypto.createHash('sha256').update(secret).digest(),iv);const encrypted=Buffer.concat([cipher.update(JSON.stringify(value)),cipher.final()]);return Buffer.concat([iv,cipher.getAuthTag(),encrypted]).toString('base64')};
 const fixtureValue={scope:'personal-student-v1',owner:id,studentId:'fixture-student',accessToken:'invalid-no-jwt-no-school-call'};
 for(const value of [fixtureValue,{...fixtureValue,owner:crypto.randomUUID()}]){
  const saved=await admin.from('eduset_student_connections').upsert({user_id:id,student_id:'fixture-student',token_enc:seal(value),expires_at:new Date(Date.now()+3600000).toISOString()});assert.ifError(saved.error);
  const invalidSaved=await api('/api/study/student',token);assert.equal(invalidSaved.status,401);assert.equal(invalidSaved.body.code,'school_connection_required');
  const after=await admin.from('eduset_student_connections').select('user_id').eq('user_id',id);assert.ifError(after.error);assert.equal(after.data.length,0);
 }
 const savedExpired=await admin.from('eduset_student_connections').upsert({user_id:id,student_id:'fixture-student',token_enc:seal(fixtureValue),expires_at:new Date(Date.now()-60000).toISOString()});assert.ifError(savedExpired.error);
 assert.equal((await api('/api/study/student',token,{action:'disconnect'})).status,200);
 const remaining=await admin.from('eduset_student_connections').select('user_id').eq('user_id',id);assert.ifError(remaining.error);assert.equal(remaining.data.length,0);
 console.log('PASS encrypted private connection storage: authenticated reads denied, malformed school sessions cleared, cross-owner ciphertext rejected, expired connection disconnect works. No school requests made.');
 assert.equal((await api('/api/study/link',token,{action:'start'},'https://evil.invalid')).status,403);
 const start=await api('/api/study/link',token,{action:'start'});assert.equal(start.status,200,JSON.stringify(start));assert.match(start.body.id,/^[A-F0-9]{16}$/);
 const poll=await api('/api/study/link',token,{action:'poll',id:start.body.id});assert.equal(poll.status,200);assert.equal(poll.body.ready,false);assert.ok(!('accessToken' in poll.body));
 assert.equal((await api('/api/study/link',token,{action:'cancel',id:start.body.id})).status,200);
 const signout=await client.auth.signOut();assert.ifError(signout.error);
 assert.equal((await api('/api/study/link',token,{action:'start'})).status,401);assert.equal((await api('/api/study/student',token)).status,401);
 console.log('PASS live local auth: anonymous denial, approved non-admin identity, personal school connection requirement, origin checks, handoff start/poll/cancel and immediate logout revocation. No school login or application writes sent.');
}finally{if(created){const {error}=await admin.auth.admin.deleteUser(id);assert.ifError(error);assert.equal(sql(`SELECT count(*) FROM auth.users WHERE id='${id}'`).trim(),'0')}console.log('Temporary local CBGPU fixture removed; no real account or school record changed.')}})().catch(e=>{console.error(e);process.exitCode=1});
