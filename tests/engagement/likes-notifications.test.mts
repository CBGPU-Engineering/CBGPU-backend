/** Synthetic, network-less PostgreSQL only; never connects to an existing database. */
import test from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import fs from 'node:fs';
const name=`cbgpu-engagement-test-${process.pid}`;
const db=(sql:string)=>execFileSync('docker',['exec','-i',name,'psql','-X','-qAt','-v','ON_ERROR_STOP=1','-U','postgres','-d','postgres'],{input:sql,encoding:'utf8',stdio:['pipe','pipe','pipe']}).trim();
const mig=(file:string)=>fs.readFileSync(new URL(`../../supabase/migrations/${file}`,import.meta.url),'utf8');
const U={a:'11111111-1111-4111-8111-111111111111',b:'22222222-2222-4222-8222-222222222222',c:'33333333-3333-4333-8333-333333333333',author:'44444444-4444-4444-8444-444444444444'};
const SID='bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const M={root:'aaaaaaaa-0000-4000-8000-000000000001',reply:'aaaaaaaa-0000-4000-8000-000000000002',hidden:'aaaaaaaa-0000-4000-8000-000000000003',other:'aaaaaaaa-0000-4000-8000-000000000004'};
const as=(user:string,session=true)=>`set role authenticated;set request.jwt.claims='${JSON.stringify({sub:user,role:'authenticated',...(session?{session_id:SID}:{})})}';`;
const json=(sql:string)=>JSON.parse(db(sql));
const fails=(sql:string,pattern:RegExp)=>assert.throws(()=>db(sql),(e:any)=>pattern.test(String(e.stderr||e.message)));
const like=(user:string,id:string,liked=true)=>json(as(user)+`select public.cbgpu_message_like_set('${id}',${liked});`);

test('likes: same visibility as reading the message, anonymous, idempotent, counted in every message JSON; notification inbox: own rows only, server-written, deduped, bounded',async()=>{
 execFileSync('docker',['run','--detach','--name',name,'--network','none','--env','POSTGRES_PASSWORD=synthetic-engagement-test','postgres:17-alpine','postgres','-c','listen_addresses='],{stdio:'pipe'});
 try{
  let ready=false;for(let i=0;i<200&&!ready;i++){const logs=execFileSync('docker',['logs',name],{encoding:'utf8',stdio:['ignore','pipe','pipe']});if(logs.includes('PostgreSQL init process complete; ready for start up.')){try{db('select 1;');ready=true}catch{}}if(!ready)execFileSync('sleep',['0.25'])}
  assert.ok(ready,'postgres did not start');
  db(`create role authenticated;create role anon;create role service_role bypassrls;
   create schema auth;create table auth.users(id uuid primary key);
   create function auth.uid() returns uuid language sql stable as $$select (nullif(current_setting('request.jwt.claims',true),'')::jsonb->>'sub')::uuid$$;
   create table public.profiles(id uuid primary key references auth.users(id),full_name text,avatar_url text);
   create function public.cbgpu_personal_project_identity() returns boolean language sql stable security definer set search_path='' as $$select auth.uid() is not null and (nullif(current_setting('request.jwt.claims',true),'')::jsonb->>'session_id') is not null$$;
   grant usage on schema public,auth to authenticated,anon,service_role;grant select on public.profiles to authenticated;
   insert into auth.users(id) values('${U.a}'),('${U.b}'),('${U.c}'),('${U.author}');
   insert into public.profiles(id,full_name) values('${U.a}','Alice'),('${U.b}','Bob'),('${U.c}','Cara'),('${U.author}','Author');
   create schema cbgpu_schools;
   create table cbgpu_schools.messages(id uuid primary key,user_id uuid not null references auth.users(id),scope text not null,school_id text,body text not null,created_at timestamptz not null default now(),deleted_at timestamptz,client_message_id uuid,parent_id uuid references cbgpu_schools.messages(id));
   create table cbgpu_schools.message_mutes(user_id uuid,muted_user_id uuid,created_at timestamptz default now());
   create table cbgpu_schools.direct_messages(id uuid,user_id uuid,body text);create table cbgpu_schools.memberships(id uuid,user_id uuid,school_id text);create table cbgpu_schools.contest_operator_nominations(id uuid,user_id uuid,school_id text);
   create table public.test_community(user_id uuid,scope text);
   create function cbgpu_schools.community_allowed(uid uuid,p_scope text,school text) returns boolean language sql stable security definer set search_path='' as $$select exists(select from public.test_community t where t.user_id=uid and t.scope=p_scope)$$;
   create function cbgpu_schools.message_json(m cbgpu_schools.messages) returns jsonb language sql stable set search_path='' as $$select jsonb_build_object('id',m.id,'user_id',m.user_id,'sender_label','Member '||left(replace(m.user_id::text,'-',''),8),'body',m.body,'parent_id',m.parent_id)$$;
   create function cbgpu_schools.dm_message_json(m cbgpu_schools.direct_messages) returns jsonb language sql stable security definer set search_path='' as $$select '{}'::jsonb$$;
   create function cbgpu_schools.member_json(m cbgpu_schools.memberships) returns jsonb language sql stable security definer set search_path='' as $$select '{}'::jsonb$$;
   create function cbgpu_schools.operator_nomination(n cbgpu_schools.contest_operator_nominations) returns jsonb language sql stable security definer set search_path='' as $$select '{}'::jsonb$$;
   create function cbgpu_schools.gpu_admin_booking_item(school text,booking_row jsonb) returns jsonb language sql stable security definer set search_path='' as $$select '{}'::jsonb$$;
   revoke all on function cbgpu_schools.message_json(cbgpu_schools.messages) from public;
   insert into public.test_community values('${U.a}','school-x'),('${U.b}','school-x'),('${U.author}','school-x'),('${U.c}','school-y');
   insert into cbgpu_schools.messages(id,user_id,scope,school_id,body,parent_id,deleted_at) values
    ('${M.root}','${U.author}','school-x','x','root',null,null),('${M.reply}','${U.author}','school-x','x','reply','${M.root}',null),
    ('${M.hidden}','${U.author}','school-x','x','deleted',null,now()),('${M.other}','${U.author}','school-y','y','other school',null,null);`);
  db(mig('20261126120000_default_avatars.sql'));db(mig('20261126150000_avatar_surfaces.sql'));
  const third=mig('20261126170000_message_likes_and_notifications.sql');db(third);

  // ---------------- likes
  assert.deepEqual(like(U.a,M.root),{message_id:M.root,liked:true,like_count:1});
  assert.equal(like(U.a,M.root).like_count,1,'liking twice is idempotent');
  assert.equal(like(U.b,M.root).like_count,2);
  assert.equal(like(U.a,M.reply).like_count,1,'replies can be liked too');
  assert.equal(like(U.b,M.root,false).like_count,1,'unlike');
  assert.equal(like(U.b,M.root,false).like_count,1,'unliking twice is idempotent');
  for(const [who,id,why] of [[U.c,M.root,'not allowed in that community scope'],[U.a,M.hidden,'deleted message'],[U.a,M.other,'other school scope'],[U.a,'aaaaaaaa-0000-4000-8000-0000000000ff','unknown id']])
   try{db(as(who)+`select public.cbgpu_message_like_set('${id}',true);`);assert.fail('accepted: '+why)}catch(e:any){assert.match(String(e.stderr||e.message),/MESSAGE_NOT_FOUND/,why)}
  db(`insert into cbgpu_schools.message_mutes(user_id,muted_user_id) values('${U.b}','${U.author}');`);
  fails(as(U.b)+`select public.cbgpu_message_like_set('${M.root}',true);`,/MESSAGE_NOT_FOUND/);
  assert.equal(like(U.a,M.root).like_count,1,'a mute only hides the message from the muter');
  db(`update cbgpu_schools.messages set deleted_at=now() where id='${M.root}';`);
  fails(as(U.a)+`select public.cbgpu_message_like_set('${M.reply}',true);`,/MESSAGE_NOT_FOUND/);   // a reply is judged through its root
  db(`update cbgpu_schools.messages set deleted_at=null where id='${M.root}';delete from cbgpu_schools.message_mutes;`);
  fails(as(U.a,false)+`select public.cbgpu_message_like_set('${M.root}',true);`,/AUTHENTICATION_REQUIRED/);
  fails(`set role anon;select public.cbgpu_message_like_set('${M.root}',true);`,/permission denied/);
  fails(as(U.a)+`select public.cbgpu_message_like_set(null,true);`,/INVALID_REQUEST/);

  // every message JSON carries the counts, still has the avatar and the original keys; nobody can see who liked
  like(U.b,M.root);
  const row=(user:string,id:string)=>json(as(user)+`reset role;set request.jwt.claims='${JSON.stringify({sub:user,session_id:SID})}';select cbgpu_schools.message_json(m) from cbgpu_schools.messages m where id='${id}';`);
  const a=row(U.a,M.root),b=row(U.b,M.root);
  assert.equal(a.like_count,2);assert.equal(a.liked_by_me,true);assert.equal(row(U.author,M.root).liked_by_me,false);
  assert.equal(a.sender_label,'Member 44444444');assert.ok(a.avatarUrl,'avatar fields from the previous migration are kept');assert.equal(a.body,'root');
  assert.deepEqual(Object.keys(a).sort(),['avatarUrl','body','like_count','liked_by_me','parent_id','profileImageUrl','sender_label','id','user_id'].sort());
  fails(as(U.a)+'select * from cbgpu_schools.message_likes;',/permission denied/);
  fails(as(U.a)+'select count(*) from public.profiles where id in (select user_id from cbgpu_schools.message_likes);',/permission denied/);
  const bulk=json(as(U.a)+`select jsonb_agg(to_jsonb(x) order by like_count desc) from public.cbgpu_message_likes(array['${M.root}','${M.reply}','${M.hidden}','${M.other}']::uuid[]) x;`);
  assert.deepEqual(bulk.map((x:any)=>x.message_id).sort(),[M.reply,M.root].sort(),'only readable messages are answered');
  fails(as(U.a)+`select * from public.cbgpu_message_likes((select array_agg(gen_random_uuid()) from generate_series(1,201)));`,/INVALID_REQUEST/);
  // rate limit: 60 new likes per minute per account
  db(`insert into cbgpu_schools.messages(id,user_id,scope,school_id,body) select gen_random_uuid(),'${U.author}','school-x','x','m'||g from generate_series(1,70) g;`);
  const ids=db(`select string_agg(id::text,',') from (select id from cbgpu_schools.messages where body like 'm%' order by body limit 70) s;`).split(',');
  let limited=false;for(const id of ids){try{like(U.c==='x'?U.a:U.a,id)}catch(e:any){limited=/LIKE_RATE_LIMIT/.test(String(e.stderr));break}}
  assert.ok(limited,'the 61st like within a minute is refused');
  db(`delete from cbgpu_schools.messages where id='${M.reply}';`);
  assert.equal(db(`select count(*) from cbgpu_schools.message_likes where message_id='${M.reply}';`),'0','deleting a message removes its likes');

  // ---------------- notification inbox
  const notify=(user:string,extra='')=>db(`set role service_role;select public.cbgpu_notify('${user}','selfstudy_reminder','자습신청 알림','3교시 자습신청이 확인되지 않았어요.','/study?reminder=1','{"k":"v"}'::jsonb${extra});`);
  const id1=notify(U.a,",'dedupe-1'");
  assert.match(id1,/^[0-9a-f-]{36}$/);
  assert.equal(notify(U.a,",'dedupe-1'"),'','the same event for the same account is stored once');
  notify(U.b);notify(U.a,",'dedupe-2'");
  assert.equal(db(`select count(*) from public.notifications where user_id='${U.a}';`),'2');
  assert.equal(db(as(U.a)+`select count(*) from public.notifications;`),'2','a person sees only their own rows');
  assert.equal(db(as(U.b)+`select count(*) from public.notifications;`),'1');
  assert.equal(db(as(U.a,false)+`select count(*) from public.notifications;`),'0','no fresh session, no rows');
  fails(`set role anon;select * from public.notifications;`,/permission denied/);
  fails(as(U.a)+`select public.cbgpu_notify('${U.a}','x','t');`,/permission denied/);
  fails(as(U.a)+`insert into public.notifications(user_id,kind,title) values('${U.a}','x','forged');`,/permission denied/);
  fails(as(U.a)+`update public.notifications set read_at=now();`,/permission denied/);
  const first=json(as(U.a)+`select to_jsonb(n) from public.notifications n where dedupe_key='dedupe-1';`);
  assert.deepEqual(Object.keys(first).sort(),['body','created_at','data','dedupe_key','id','kind','read_at','route','title','user_id']);
  assert.equal(first.read_at,null);assert.equal(first.route,'/study?reminder=1');assert.deepEqual(first.data,{k:'v'});
  assert.equal(db(as(U.a)+`select public.cbgpu_notifications_unread_count();`),'2');
  assert.equal(db(as(U.a)+`select public.cbgpu_notifications_mark_read(array['${first.id}']::uuid[]);`),'1');
  assert.equal(db(as(U.a)+`select public.cbgpu_notifications_unread_count();`),'1');
  assert.equal(db(as(U.a)+`select public.cbgpu_notifications_mark_read(array['${first.id}']::uuid[]);`),'0','already read');
  const bId=db(`select id from public.notifications where user_id='${U.b}';`);
  assert.equal(db(as(U.a)+`select public.cbgpu_notifications_mark_read(array['${bId}']::uuid[]);`),'0','cannot mark someone else’s notification');
  assert.equal(db(as(U.a)+`select public.cbgpu_notifications_mark_read();`),'1','no argument marks everything of mine');
  assert.equal(db(as(U.b)+`select public.cbgpu_notifications_unread_count();`),'1','…and nobody else’s');
  fails(as(U.a,false)+`select public.cbgpu_notifications_unread_count();`,/AUTHENTICATION_REQUIRED/);
  fails(`set role anon;select public.cbgpu_notifications_unread_count();`,/permission denied/);
  assert.equal(db(as(U.a)+`delete from public.notifications where id='${bId}' returning 1;`),'','cannot delete someone else’s');
  assert.equal(db(as(U.a)+`delete from public.notifications where id='${first.id}' returning 1;`),'1');
  // input rules (also protect the table from a buggy producer)
  for(const bad of [`'Bad Kind','t'`,`'ok',''`,`'ok','${'x'.repeat(121)}'`])fails(`set role service_role;select public.cbgpu_notify('${U.a}',${bad});`,/violates check constraint/);
  fails(`set role service_role;select public.cbgpu_notify('${U.a}','ok','t','b','https://evil.example/x');`,/violates check constraint/);
  fails(`set role service_role;select public.cbgpu_notify('${U.a}','ok','t','b','javascript:alert(1)');`,/violates check constraint/);
  for(const evil of ['//evil.example','/\\evil.example'])fails(`set role service_role;select public.cbgpu_notify('${U.a}','ok','t','b','${evil.replace(/\\/g,'\\\\')}');`,/violates check constraint/);
  db(`set role service_role;select public.cbgpu_notify('${U.a}','ok','t','b','https://print.cbgpu.com/manage/manufacturing/print');`);
  fails(`set role service_role;select public.cbgpu_notify('${U.a}','ok','t','b',null,'[1]'::jsonb);`,/violates check constraint/);
  // bounded history: at most 500 per account
  db(`set role service_role;select public.cbgpu_notify('${U.c}','bulk','n'||g) from generate_series(1,505) g;`);
  assert.equal(db(`select count(*) from public.notifications where user_id='${U.c}';`),'500');
  assert.equal(db(`select count(*) from public.notifications where user_id='${U.c}' and title='n1';`),'0','the oldest are dropped first');
  db(`delete from public.profiles where id='${U.c}';delete from auth.users where id='${U.c}';`);
  assert.equal(db(`select count(*) from public.notifications where user_id='${U.c}';`),'0','deleting the account removes its inbox');

  // re-running the migration changes nothing
  const before=db(`select (select count(*) from public.notifications)||'/'||(select count(*) from cbgpu_schools.message_likes);`);
  db(third);db(third);
  assert.equal(db(`select (select count(*) from public.notifications)||'/'||(select count(*) from cbgpu_schools.message_likes);`),before);
 }finally{
  try{execFileSync('docker',['rm','-f',name],{stdio:'pipe'})}catch{}
 }
});
