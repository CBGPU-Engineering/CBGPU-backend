/** Synthetic, network-less PostgreSQL only; never connects to an existing database. */
import test from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import fs from 'node:fs';
const name=`cbgpu-avatar-surfaces-test-${process.pid}`;
const db=(sql:string)=>execFileSync('docker',['exec','-i',name,'psql','-X','-qAt','-v','ON_ERROR_STOP=1','-U','postgres','-d','postgres'],{input:sql,encoding:'utf8',stdio:['pipe','pipe','pipe']}).trim();
const read=(file:string)=>fs.readFileSync(new URL(`../../supabase/migrations/${file}`,import.meta.url),'utf8');
const first=read('20261126120000_default_avatars.sql'),second=read('20261126150000_avatar_surfaces.sql');
const A='11111111-1111-4111-8111-111111111111',B='22222222-2222-4222-8222-222222222222',GONE='77777777-7777-4777-8777-777777777777',SB='bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const STORE='https://cbgpu.com/backend/storage/v1/object/public/avatars';
const as=(user:string,session=true)=>`set role authenticated;set request.jwt.claims='${JSON.stringify({sub:user,role:'authenticated',...(session?{session_id:SB}:{})})}';`;
const json=(sql:string)=>JSON.parse(db(sql));
const fails=(sql:string,pattern:RegExp)=>assert.throws(()=>db(sql),(e:any)=>pattern.test(String(e.stderr||e.message)));
const dicebear=(seed:string)=>`https://api.dicebear.com/9.x/thumbs/png?seed=${seed}&size=256`;
const seedOf=(id:string)=>db(`select avatar_seed from public.profiles where id='${id}';`);

test('avatars on every response builder, avatar-only cross-school lookup, storage-only avatar_url',async()=>{
 execFileSync('docker',['run','--detach','--name',name,'--network','none','--env','POSTGRES_PASSWORD=synthetic-avatar-test','postgres:17-alpine','postgres','-c','listen_addresses='],{stdio:'pipe'});
 try{
  let ready=false;for(let i=0;i<200&&!ready;i++){const logs=execFileSync('docker',['logs',name],{encoding:'utf8',stdio:['ignore','pipe','pipe']});if(logs.includes('PostgreSQL init process complete; ready for start up.')){try{db('select 1;');ready=true}catch{}}if(!ready)execFileSync('sleep',['0.25'])}
  assert.ok(ready,'postgres did not start');
  // Live shape: schools builders are owner-only, clients have no USAGE on cbgpu_schools, profiles are school-private.
  db(`create role authenticated;create role anon;create role service_role bypassrls;
   create schema auth;create function auth.uid() returns uuid language sql stable as $$select (nullif(current_setting('request.jwt.claims',true),'')::jsonb->>'sub')::uuid$$;
   create table public.profiles(id uuid primary key,full_name text,student_id text,avatar_url text);
   alter table public.profiles enable row level security;
   -- B shares no school with A: only a person's own row is readable.
   create policy own_only on public.profiles for select to authenticated using(id=auth.uid());
   create function public.cbgpu_personal_project_identity() returns boolean language sql stable security definer set search_path='' as $$select auth.uid() is not null and (nullif(current_setting('request.jwt.claims',true),'')::jsonb->>'session_id') is not null$$;
   grant usage on schema public,auth to authenticated,anon,service_role;grant select,insert,update on public.profiles to authenticated;grant all on public.profiles to service_role;
   create schema cbgpu_schools;
   create table cbgpu_schools.messages(id uuid,user_id uuid,body text);create table cbgpu_schools.direct_messages(id uuid,user_id uuid,body text);
   create table cbgpu_schools.memberships(id uuid,user_id uuid,school_id text);create table cbgpu_schools.contest_operator_nominations(id uuid,user_id uuid,school_id text);
   create function cbgpu_schools.message_json(m cbgpu_schools.messages) returns jsonb language sql stable set search_path='' as $$select jsonb_build_object('id',m.id,'user_id',m.user_id,'sender_label','Member '||left(replace(m.user_id::text,'-',''),8),'body',m.body)$$;
   create function cbgpu_schools.dm_message_json(m cbgpu_schools.direct_messages) returns jsonb language sql stable security definer set search_path='' as $$select jsonb_build_object('id',m.id,'user_id',m.user_id,'sender_label','dm','body',m.body)$$;
   create function cbgpu_schools.member_json(m cbgpu_schools.memberships) returns jsonb language sql stable security definer set search_path='' as $$select to_jsonb(m)||jsonb_build_object('full_name',(select p.full_name from public.profiles p where p.id=m.user_id))$$;
   create function cbgpu_schools.operator_nomination(n cbgpu_schools.contest_operator_nominations) returns jsonb language sql stable security definer set search_path='' as $$select jsonb_build_object('id',n.id,'display_name',(select p.full_name from public.profiles p where p.id=n.user_id))$$;
   create function cbgpu_schools.gpu_admin_booking_item(school text,booking_row jsonb) returns jsonb language sql stable security definer set search_path='' as $$select jsonb_build_object('id',booking_row->>'id','school',school,'full_name',booking_row->>'full_name')$$;
   revoke all on function cbgpu_schools.message_json(cbgpu_schools.messages),cbgpu_schools.dm_message_json(cbgpu_schools.direct_messages),cbgpu_schools.member_json(cbgpu_schools.memberships),cbgpu_schools.operator_nomination(cbgpu_schools.contest_operator_nominations),cbgpu_schools.gpu_admin_booking_item(text,jsonb) from public;
   insert into public.profiles(id,full_name,student_id) values('${A}','Alice Kim','20230001'),('${B}','Bob Lee','20230002');`);
  db(first);
  const probes=(user:string)=>({
   message:json(as(user)+`reset role;select cbgpu_schools.message_json(row('${'1'.repeat(8)}-1111-4111-8111-111111111111','${A}','hi')::cbgpu_schools.messages);`),
   dm:json(`select cbgpu_schools.dm_message_json(row('${'1'.repeat(8)}-1111-4111-8111-111111111111','${A}','yo')::cbgpu_schools.direct_messages);`),
   member:json(`select cbgpu_schools.member_json(row('${'1'.repeat(8)}-1111-4111-8111-111111111111','${A}','x')::cbgpu_schools.memberships);`),
   nomination:json(`select cbgpu_schools.operator_nomination(row('${'1'.repeat(8)}-1111-4111-8111-111111111111','${A}','x')::cbgpu_schools.contest_operator_nominations);`),
   booking:json(`select cbgpu_schools.gpu_admin_booking_item('x','{"id":"b1","owner_id":"${A}","full_name":"Alice Kim"}'::jsonb);`),
  });
  const before=probes(B);
  db(second);
  const after=probes(B),seedA=seedOf(A);
  for(const key of Object.keys(before) as (keyof typeof before)[]){
   const {avatarUrl,profileImageUrl,...rest}=after[key];
   assert.deepEqual(rest,before[key],`${key}: every original field is unchanged`);
   assert.equal(avatarUrl,dicebear(seedA),`${key}: avatarUrl is the server-built default`);
   assert.equal(profileImageUrl,null,`${key}: other people's photo is never exposed`);
  }
  assert.equal(after.message.sender_label,'Member 11111111','anonymous community labels are untouched');
  // an account with no profile row still gets a non-null avatar
  const gone=json(`select cbgpu_schools.message_json(row('${'1'.repeat(8)}-1111-4111-8111-111111111111','${GONE}','x')::cbgpu_schools.messages);`);
  assert.match(gone.avatarUrl,/^https:\/\/api\.dicebear\.com\/9\.x\/thumbs\/png\?seed=00000000-0000-0000-0000-000000000000&size=256$/);
  // owner sees their own photo flag; photo first, deletion returns the same default
  db(`update public.profiles set avatar_url='${STORE}/${A}/avatar.jpg?v=1' where id='${A}';`);
  const selfPhoto=json(`set request.jwt.claims='${JSON.stringify({sub:A,session_id:SB})}';select cbgpu_schools.message_json(row('${'1'.repeat(8)}-1111-4111-8111-111111111111','${A}','hi')::cbgpu_schools.messages);`);
  assert.equal(selfPhoto.avatarUrl,`${STORE}/${A}/avatar.jpg?v=1`);assert.equal(selfPhoto.profileImageUrl,`${STORE}/${A}/avatar.jpg?v=1`);
  const otherSees=probes(B).message;assert.equal(otherSees.avatarUrl,`${STORE}/${A}/avatar.jpg?v=1`);assert.equal(otherSees.profileImageUrl,null);
  db(`update public.profiles set avatar_url=null where id='${A}';`);
  assert.equal(probes(B).message.avatarUrl,dicebear(seedA),'after deleting the photo the same default returns');

  // builders stay owner-only and keep their security mode; helper is not reachable by clients
  for(const fn of ['message_json','dm_message_json','member_json','operator_nomination','gpu_admin_booking_item','avatar_fields'])
   assert.equal(db(`select has_function_privilege('authenticated',p.oid,'execute')or has_function_privilege('anon',p.oid,'execute')or has_function_privilege('service_role',p.oid,'execute') from pg_proc p join pg_namespace n on n.oid=p.pronamespace where n.nspname='cbgpu_schools' and p.proname='${fn}';`),'f',`${fn} stays owner-only`);
  assert.equal(db(`select string_agg(proname||'='||prosecdef,' ' order by proname) from pg_proc p join pg_namespace n on n.oid=p.pronamespace where n.nspname='cbgpu_schools' and proname in('message_json','dm_message_json','member_json','operator_nomination','gpu_admin_booking_item');`),'dm_message_json=true gpu_admin_booking_item=true member_json=true message_json=false operator_nomination=true');

  // ---- school A user seen from school B: avatarUrl only, nothing else
  assert.equal(db(as(B)+`select count(*) from public.profiles where id='${A}';`),'0','B cannot read A through the normal (school-private) path');
  const rows=json(as(B)+`select jsonb_agg(to_jsonb(u)) from public.cbgpu_avatar_urls(array['${A}']::uuid[]) u;`);
  assert.deepEqual(Object.keys(rows[0]).sort(),['avatarUrl','id'],'only the id and avatarUrl cross the school boundary');
  assert.equal(rows[0].avatarUrl,dicebear(seedA));
  for(const secret of['Alice','Kim','20230001'])assert.ok(!JSON.stringify(rows).includes(secret),`${secret} must not cross the boundary`);
  db(`update public.profiles set avatar_url='${STORE}/${A}/avatar.jpg?v=2' where id='${A}';`);
  assert.equal(json(as(B)+`select jsonb_agg(to_jsonb(u)) from public.cbgpu_avatar_urls(array['${A}']::uuid[]) u;`)[0].avatarUrl,`${STORE}/${A}/avatar.jpg?v=2`,'an uploaded photo is what B sees');
  db(`update public.profiles set avatar_url=null where id='${A}';`);
  assert.equal(json(as(B)+`select jsonb_agg(to_jsonb(u)) from public.cbgpu_avatar_urls(array['${A}']::uuid[]) u;`)[0].avatarUrl,dicebear(seedA),'after deletion B sees the same default again');
  fails(as(B,false)+`select * from public.cbgpu_avatar_urls(array['${A}']::uuid[]);`,/AUTHENTICATION_REQUIRED/);
  fails(`set role anon;select * from public.cbgpu_avatar_urls(array['${A}']::uuid[]);`,/permission denied/);
  fails(as(B)+`select * from public.cbgpu_avatar_urls((select array_agg(gen_random_uuid()) from generate_series(1,501)));`,/INVALID_AVATAR_REQUEST/);

  // ---- avatar_url can only point at the account's own file in our storage
  for(const ok of[`'${STORE}/${A}/avatar.jpg?v=1795003809180'`,`'${STORE}/${A}/avatar.jpeg?v=1'`,`'${STORE}/${A}/avatar.webp'`,'null'])db(`update public.profiles set avatar_url=${ok} where id='${A}';`);
  for(const bad of[`'${STORE}/${B}/avatar.jpg'`,"'https://evil.example/pixel.png'","'https://cbgpu.com/backend/storage/v1/object/public/private/"+A+"/avatar.jpg'",`'http://cbgpu.com/backend/storage/v1/object/public/avatars/${A}/avatar.jpg'`,"'javascript:alert(1)'",`'${STORE}/${A}/avatar.svg'`,`'${STORE}/${A}/../${B}/avatar.jpg'`,"'   '",`'${STORE}/${A}/avatar.jpg?v=1&x=<script>'`])
   fails(`update public.profiles set avatar_url=${bad} where id='${A}';`,/profiles_avatar_url_storage_chk/);
  db(`update public.profiles set avatar_url=null where id='${A}';`);

  // ---- re-running changes nothing
  const state=db('select jsonb_agg(to_jsonb(p) order by id) from public.profiles p;');
  db(second);db(second);
  assert.equal(db('select jsonb_agg(to_jsonb(p) order by id) from public.profiles p;'),state);
  assert.equal(db("select count(*) from pg_proc where proname like '%\\_core\\_core' escape '\\';"),'0');
  assert.deepEqual(probes(B).message,after.message);

  // ---- the rollback written in docs/avatars-rollout.md really restores the originals (and can be re-applied)
  const doc=fs.readFileSync(new URL('../../docs/avatars-rollout.md',import.meta.url),'utf8');
  const rollback=[...doc.matchAll(/```sql\n([\s\S]*?)```/g)].map(match=>match[1]).find(block=>block.includes('_core'));
  assert.ok(rollback,'the documented rollback block exists');
  db(rollback!);
  assert.deepEqual(probes(B),before,'after the documented rollback every builder returns exactly what it did before');
  assert.equal(db("select count(*) from pg_proc where proname in('avatar_fields','cbgpu_avatar_urls') or proname like '%\\_core' escape '\\';"),'0');
  db(`update public.profiles set avatar_url='https://evil.example/x.png' where id='${A}';update public.profiles set avatar_url=null where id='${A}';`); // the check is gone with the rollback
  db(second);                                                   // ...and the migration can be applied again
  assert.deepEqual(probes(B).message,after.message,'re-applying after a rollback gives the same result');
  fails(`update public.profiles set avatar_url='https://evil.example/x.png' where id='${A}';`,/profiles_avatar_url_storage_chk/);
 }finally{
  try{execFileSync('docker',['rm','-f',name],{stdio:'pipe'})}catch{}
 }
});
