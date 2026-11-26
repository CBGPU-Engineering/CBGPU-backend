/** Synthetic, network-less PostgreSQL only; never connects to an existing database. */
import test from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import fs from 'node:fs';
const name=`cbgpu-avatars-test-${process.pid}`;
const db=(sql:string)=>execFileSync('docker',['exec','-i',name,'psql','-X','-qAt','-v','ON_ERROR_STOP=1','-U','postgres','-d','postgres'],{input:sql,encoding:'utf8',stdio:['pipe','pipe','pipe']}).trim();
const migration=fs.readFileSync(new URL('../../supabase/migrations/20261126120000_default_avatars.sql',import.meta.url),'utf8');
const ids={a:'11111111-1111-4111-8111-111111111111',b:'22222222-2222-4222-8222-222222222222',c:'33333333-3333-4333-8333-333333333333',hidden:'99999999-9999-4999-8999-999999999999',fresh:'44444444-4444-4444-8444-444444444444'};
const as=(user:string)=>`set role authenticated;set request.jwt.claims='${JSON.stringify({sub:user,role:'authenticated'})}';`;
const card=(who:string,id:string)=>JSON.parse(db(as(who)+`select to_jsonb(c) from public.user_cards c where id='${id}';`));
const dicebear=(seed:string)=>`https://api.dicebear.com/9.x/thumbs/png?seed=${seed}&size=256`;
const serverDefault=(id:string)=>db(`select public.cbgpu_default_avatar_url(avatar_seed) from public.profiles where id='${id}';`);
const seedOf=(id:string)=>db(`select avatar_seed from public.profiles where id='${id}';`);
const fails=(sql:string,pattern:RegExp)=>assert.throws(()=>db(sql),(e:any)=>pattern.test(String(e.stderr||e.message)));

test('default avatars: existing users, photo precedence, deletion, immutability, settings, privacy',async()=>{
 execFileSync('docker',['run','--detach','--name',name,'--network','none','--env','POSTGRES_PASSWORD=synthetic-avatar-test','postgres:17-alpine','postgres','-c','listen_addresses='],{stdio:'pipe'});
 try{
  let ready=false;for(let i=0;i<200&&!ready;i++){const logs=execFileSync('docker',['logs',name],{encoding:'utf8',stdio:['ignore','pipe','pipe']});if(logs.includes('PostgreSQL init process complete; ready for start up.')){try{db('select 1;');ready=true}catch{}}if(!ready)execFileSync('sleep',['0.25'])}
  assert.ok(ready,'postgres did not start');
  // Live shape: table-level grants (no column restriction), own-row update, a row hidden by RLS.
  db(`create role authenticated;create role anon;create role service_role bypassrls;
   create schema auth;create function auth.uid() returns uuid language sql stable as $$select (nullif(current_setting('request.jwt.claims',true),'')::jsonb->>'sub')::uuid$$;
   create table public.profiles(id uuid primary key,full_name text,student_id text,avatar_url text,is_admin boolean default false);
   alter table public.profiles enable row level security;
   create policy read_visible on public.profiles for select to authenticated using(id<>'${ids.hidden}');
   create policy update_own on public.profiles for update to authenticated using(auth.uid()=id) with check(auth.uid()=id);
   create policy insert_own on public.profiles for insert to authenticated with check(auth.uid()=id);
   create function public.cbgpu_study_member_profiles(p_user_ids uuid[]) returns table(id uuid,full_name text) language sql stable security definer set search_path='' as $$select p.id,p.full_name from public.profiles p where p.id=any(p_user_ids) and p.id<>'${ids.hidden}'$$;
   grant usage on schema public,auth to authenticated,anon,service_role;
   grant select,insert,update on public.profiles to authenticated;grant all on public.profiles to service_role;
   insert into public.profiles(id,full_name,student_id) values('${ids.a}','Kim Minseo','20231234'),('${ids.b}','Lee Jiho','20231235'),('${ids.c}','Park Seoyeon',null),('${ids.hidden}','Hidden User',null);
   update public.profiles set avatar_url='https://files.example/avatars/${ids.b}/avatar.jpg?v=1' where id='${ids.b}';`);

  // ---- existing users are migrated: every row gets its own seed
  db(migration);
  assert.equal(db('select count(*) from public.profiles where avatar_seed is null;'),'0');
  assert.equal(db('select count(distinct avatar_seed)=count(*) from public.profiles;'),'t');
  const seeds=Object.fromEntries(Object.values(ids).slice(0,3).map(id=>[id,seedOf(id)]));

  // ---- no photo -> DiceBear default (exact URL contract), photo -> photo first
  const a=card(ids.a,ids.a);
  assert.deepEqual(Object.keys(a).sort(),['avatarUrl','id','name','profileImageUrl'],'the card carries exactly the agreed fields');
  assert.equal(a.profileImageUrl,null);
  assert.equal(a.avatarUrl,dicebear(seeds[ids.a]),'no photo -> server-built DiceBear URL from the opaque seed');
  const b=card(ids.a,ids.b);
  assert.equal(b.profileImageUrl,`https://files.example/avatars/${ids.b}/avatar.jpg?v=1`);
  assert.equal(b.avatarUrl,b.profileImageUrl);
  assert.equal(serverDefault(ids.b),dicebear(seeds[ids.b]),'the default for this account is unchanged while a photo is set');

  // ---- the URL carries only the opaque seed: no name, student number or e-mail-like data
  for(const id of [ids.a,ids.b,ids.c]){
   const url=serverDefault(id);
   assert.match(url,/^https:\/\/api\.dicebear\.com\/9\.x\/thumbs\/png\?seed=[0-9a-f-]{36}&size=256$/);
   for(const pii of['Kim','Lee','Park','Minseo','2023','@'])assert.ok(!url.includes(pii),`URL leaks ${pii}`);
  }

  // ---- uploading, then deleting, a photo: same default again; seed never changes
  db(as(ids.a)+`update public.profiles set avatar_url='https://files.example/avatars/${ids.a}/avatar.jpg?v=2' where id='${ids.a}';`);
  assert.equal(card(ids.a,ids.a).avatarUrl,`https://files.example/avatars/${ids.a}/avatar.jpg?v=2`);
  db(as(ids.a)+`update public.profiles set avatar_url=null where id='${ids.a}';`);
  assert.deepEqual(card(ids.a,ids.a),a,'after deleting the photo the card is exactly the original default');
  db(as(ids.a)+`update public.profiles set avatar_url='   ' where id='${ids.a}';`);
  assert.equal(card(ids.a,ids.a).profileImageUrl,null,'blank photo counts as none');
  assert.equal(card(ids.a,ids.a).avatarUrl,a.avatarUrl);
  db(as(ids.a)+`update public.profiles set avatar_url=null where id='${ids.a}';`);
  assert.equal(seedOf(ids.a),seeds[ids.a]);

  // ---- computed columns agree with the view
  assert.equal(db(as(ids.a)+`select p.display_avatar_url||'|'||coalesce(p.profile_image_url,'null') from public.profiles p where p.id='${ids.b}';`),`${b.avatarUrl}|${b.profileImageUrl}`);

  // ---- the seed can never change, whoever asks; clients cannot choose one at signup either
  fails(as(ids.a)+`update public.profiles set avatar_seed=gen_random_uuid() where id='${ids.a}';`,/AVATAR_SEED_IMMUTABLE/);
  fails(`update public.profiles set avatar_seed=gen_random_uuid() where id='${ids.a}';`,/AVATAR_SEED_IMMUTABLE/);
  fails(`set role service_role;update public.profiles set avatar_seed=gen_random_uuid() where id='${ids.a}';`,/AVATAR_SEED_IMMUTABLE/);
  const chosen='deadbeef-dead-4eef-8eef-deadbeefdead';
  db(as(ids.fresh)+`insert into public.profiles(id,full_name,avatar_seed) values('${ids.fresh}','New User','${chosen}');`);
  assert.notEqual(seedOf(ids.fresh),chosen,'a client-chosen seed is replaced by a random one');
  assert.match(card(ids.fresh,ids.fresh).avatarUrl,/seed=[0-9a-f-]{36}&size=256$/);
  const trusted='cafef00d-cafe-4f00-8f00-cafef00dcafe';
  db(`insert into public.profiles(id,full_name,avatar_seed) values('55555555-5555-4555-8555-555555555555','Restored','${trusted}');`);
  assert.equal(seedOf('55555555-5555-4555-8555-555555555555'),trusted,'trusted restores keep the stored seed');

  // ---- re-running the migration is a no-op: nothing changes for anyone
  const before=db('select jsonb_agg(to_jsonb(p) order by id) from public.profiles p;');
  db(migration);db(migration);
  assert.equal(db('select jsonb_agg(to_jsonb(p) order by id) from public.profiles p;'),before);

  // ---- one server-side setting controls style/version/format/size for everyone
  db(`update public.avatar_settings set style='bottts-neutral',version='9.x',format='svg',size=128;`);
  assert.equal(card(ids.a,ids.a).avatarUrl,`https://api.dicebear.com/9.x/bottts-neutral/svg?seed=${seeds[ids.a]}&size=128`);
  assert.equal(card(ids.a,ids.c).avatarUrl,`https://api.dicebear.com/9.x/bottts-neutral/svg?seed=${seeds[ids.c]}&size=128`);
  db(`update public.avatar_settings set base_url='https://img.example.org/avatars';`);
  assert.equal(card(ids.a,ids.c).avatarUrl,`https://img.example.org/avatars/9.x/bottts-neutral/svg?seed=${seeds[ids.c]}&size=128`,'switching to an own proxy is a config change');
  for(const bad of["base_url='http://insecure.example'","base_url='https://x.example/a?b=1'","style='../x'","version='latest'","format='gif'","size=100000"])fails(`update public.avatar_settings set ${bad};`,/violates check constraint/);
  db(`update public.avatar_settings set base_url='https://api.dicebear.com',style='thumbs',format='png',size=256;`);
  assert.equal(card(ids.a,ids.a).avatarUrl,dicebear(seeds[ids.a]));

  // ---- scoped screens: same visibility as the existing RPC, avatar fields added, never a raw seed
  const cards=JSON.parse(db(as(ids.a)+`select coalesce(jsonb_agg(to_jsonb(c) order by name),'[]') from public.cbgpu_study_member_cards(array['${ids.a}','${ids.b}','${ids.hidden}']::uuid[]) c;`));
  assert.deepEqual(cards.map((c:any)=>c.id).sort(),[ids.a,ids.b].sort(),'the existing visibility rule still decides who is returned');
  assert.deepEqual(Object.keys(cards[0]).sort(),['avatarUrl','id','name','profileImageUrl']);
  assert.equal(cards.find((c:any)=>c.id===ids.a).avatarUrl,dicebear(seeds[ids.a]));
  assert.equal(cards.find((c:any)=>c.id===ids.b).avatarUrl,`https://files.example/avatars/${ids.b}/avatar.jpg?v=1`);
  fails('set role anon;select * from public.cbgpu_study_member_cards(array[]::uuid[]);',/permission denied/);

  // ---- boundaries: settings are server-only, anon sees nothing, profile visibility is unchanged
  fails(as(ids.a)+'select * from public.avatar_settings;',/permission denied/);
  fails(as(ids.a)+"update public.avatar_settings set style='x';",/permission denied/);
  fails('set role anon;select * from public.user_cards;',/permission denied/);
  fails("set role anon;select public.cbgpu_default_avatar_url(gen_random_uuid());",/permission denied/);
  fails(as(ids.a)+`select p.default_avatar_url from public.profiles p;`,/does not exist/);
  assert.equal(db(as(ids.a)+`select count(*) from public.user_cards where id='${ids.hidden}';`),'0','RLS still hides profiles through the view');
 }finally{
  try{execFileSync('docker',['rm','-f',name],{stdio:'pipe'})}catch{}
 }
});
