/** Real PostgreSQL RLS with live-shaped anon ACLs; no existing database or HTTP authentication. */
import test from 'node:test';import assert from 'node:assert/strict';import {execFileSync} from 'node:child_process';import fs from 'node:fs';
const name=`cbgpu-poll-anonymous-test-${process.pid}`;
const db=(sql:string)=>execFileSync('docker',['exec','-i',name,'psql','-X','-qAt','-v','ON_ERROR_STOP=1','-U','postgres','-d','postgres'],{input:sql,encoding:'utf8',stdio:['pipe','pipe','pipe']}).trim();
const tables=['polls','poll_questions','poll_responses','poll_answers'];
const migrations=['20261123162344_atomic_poll_editor_save.sql','20261123164337_atomic_poll_response_submit.sql','20261123165638_poll_result_visibility.sql'];
const latest='20261123175207_deny_anonymous_internal_polls.sql';
const actors=['11111111-1111-4111-8111-111111111111','22222222-2222-4222-8222-222222222222'];
const sessions=['aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa','bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'];
const role=(which:string,index=1)=>`set role ${which};set request.jwt.claims='${JSON.stringify({sub:actors[index],session_id:sessions[index],role:which})}';`;
const json=(value:unknown)=>`'${JSON.stringify(value).replaceAll("'","''")}'::jsonb`;
const apply=(file:string)=>db(fs.readFileSync(new URL('../../supabase/migrations/'+file,import.meta.url),'utf8'));
const snapshot=()=>tables.map(table=>db(`select coalesce(jsonb_agg(to_jsonb(row) order by row.id),'[]') from public.${table} row;`));
const policySnapshot=()=>db("select jsonb_agg(jsonb_build_object('table',tablename,'name',policyname,'roles',roles,'command',cmd,'permissive',permissive,'using',qual,'check',with_check) order by tablename,policyname) from pg_policies where schemaname='public' and tablename in ('polls','poll_questions','poll_responses','poll_answers') and policyname not like '%anonymous_boundary';");
const aclSnapshot=()=>db("select jsonb_agg(jsonb_build_object('table',relname,'acl',relacl) order by relname) from pg_class where oid in ('public.polls'::regclass,'public.poll_questions'::regclass,'public.poll_responses'::regclass,'public.poll_answers'::regclass);");
test('anon is denied by restrictive RLS even with table privileges; authenticated and maintenance behavior remains intact',async()=>{
 execFileSync('docker',['run','--detach','--name',name,'--network','none','--env','POSTGRES_PASSWORD=synthetic-anonymous-test','postgres:17-alpine','postgres','-c','listen_addresses='],{stdio:'pipe'});
 try{
  let ready=false;for(let i=0;i<200;i++){const logs=execFileSync('docker',['logs',name],{encoding:'utf8',stdio:['ignore','pipe','pipe']});if(logs.includes('PostgreSQL init process complete; ready for start up.')){try{db('select 1');ready=true;break}catch{}}await new Promise(r=>setTimeout(r,100))}assert.equal(ready,true);
  db(fs.readFileSync(new URL('./bootstrap.sql',import.meta.url),'utf8'));for(const file of migrations)apply(file);
  // Make missing ACLs incapable of masking a policy regression. These grants
  // exist only inside this owned synthetic database, never in the migration.
  db('grant all on public.polls,public.poll_questions,public.poll_responses,public.poll_answers to anon;grant select on public.profiles to anon;create role service_role bypassrls;grant all on public.polls,public.poll_questions,public.poll_responses,public.poll_answers to service_role;');
  assert.equal(db("select pg_has_role('authenticated','anon','member');"),'f');
  const created=JSON.parse(db(role('authenticated',0)+`select public.cbgpu_save_poll(null,null,${json({title:'Synthetic hidden poll',show_results:false})},${json([{id:'question',title:'Synthetic question',type:'text',required:true,options:[],config:{}}])});`));
  const question=db(`select id from public.poll_questions where poll_id='${created.id}';`);
  const response=JSON.parse(db(role('authenticated')+`select public.cbgpu_submit_poll('${created.id}',${created.editor_revision},${json({[question]:'Synthetic private answer'})});`));
  const answer=db(`select id from public.poll_answers where response_id='${response.id}';`);
  // Before the new boundary, real SELECT grants expose internal poll/question
  // rows through their legacy permissive public-read policies.
  assert.equal(db("set role anon;set request.jwt.claims='{}';select count(*) from public.polls;"),'1');
  assert.equal(db("set role anon;set request.jwt.claims='{}';select count(*) from public.poll_questions;"),'1');
  const before=snapshot(),oldPolicies=policySnapshot(),oldAcls=aclSnapshot();apply(latest);
  assert.deepEqual(snapshot(),before);assert.equal(policySnapshot(),oldPolicies);assert.equal(aclSnapshot(),oldAcls);
  for(const context of ["set role anon;set request.jwt.claims='{}';",role('anon'),role('anon',0)]){
   for(const table of tables){assert.equal(db(context+`select count(*) from public.${table};`),'0');for(const privilege of ['SELECT','INSERT','UPDATE','DELETE'])assert.equal(db(`select has_table_privilege('anon','public.${table}','${privilege}');`),'t')}
   // UPDATE/DELETE produce zero returned rows under USING(false), rather than
   // relying on an ACL exception. No stored value or row may change.
   for(const table of tables){
    const column=table==='polls'||table==='poll_questions'?'title':table==='poll_answers'?'value':'user_id';
    const assignment=column==='title'?"'anonymous overwrite'":column==='value'?"'\"anonymous overwrite\"'::jsonb":`'${actors[0]}'::uuid`;
    assert.equal(db(context+`with changed as(update public.${table} set ${column}=${assignment} returning id)select count(*) from changed;`),'0');
    assert.equal(db(context+`with removed as(delete from public.${table} returning id)select count(*) from removed;`),'0');
   }
   const inserts=[
    `insert into public.polls(title,created_by)values('anonymous insert','${actors[1]}');`,
    `insert into public.poll_questions(poll_id,type,title)values('${created.id}','text','anonymous insert');`,
    `insert into public.poll_responses(poll_id,user_id)values('${created.id}','${actors[0]}');`,
    `insert into public.poll_answers(response_id,question_id,value)values('${response.id}','${question}','"anonymous insert"');`,
   ];
   for(const sql of inserts)assert.throws(()=>db(context+sql),error=>{const message=String((error as any).stderr);assert.equal(message.includes('permission denied'),false,'Missing privilege must not mask RLS');return /row-level security|POLL_RESPONSE_QUESTION_SCOPE/.test(message)});
   assert.deepEqual(snapshot(),before);
  }
  // New policies apply only to anon, not authenticated or privileged service roles.
  for(const context of [role('authenticated'),role('authenticated',0),'set role service_role;'])for(const table of tables)assert.equal(db(context+`select count(*) from public.${table};`),'1');
  const repeated=JSON.parse(db(role('authenticated')+`select public.cbgpu_submit_poll('${created.id}',${created.editor_revision},${json({[question]:'Synthetic private answer'})});`));assert.equal(repeated.id,response.id);assert.equal(repeated.reused,true);
  const updated=JSON.parse(db(role('authenticated',0)+`select public.cbgpu_save_poll('${created.id}',${created.editor_revision},${json({title:'Synthetic hidden poll edited',show_results:false})},${json([{id:question,title:'Synthetic question edited',type:'text',required:true,options:[],config:{}}])});`));assert.ok(updated.editor_revision>created.editor_revision);assert.equal(db(`select value from public.poll_answers where id='${answer}';`),'"Synthetic private answer"');
  const newPoll=JSON.parse(db(role('authenticated',0)+`select public.cbgpu_save_poll(null,null,${json({title:'Signed-in write after boundary',show_results:true})},${json([{id:'new-question',title:'Signed-in answer',type:'text',required:true,options:[],config:{}}])});`));
  const newQuestion=db(`select id from public.poll_questions where poll_id='${newPoll.id}';`);
  const newResponse=JSON.parse(db(role('authenticated')+`select public.cbgpu_submit_poll('${newPoll.id}',${newPoll.editor_revision},${json({[newQuestion]:'Fresh authenticated submission'})});`));assert.equal(newResponse.submitted,true);assert.equal(db(`select count(*) from public.poll_answers where response_id='${newResponse.id}';`),'1');
  const afterAuthenticatedEdit=snapshot();for(let i=0;i<2;i++)apply(latest);assert.deepEqual(snapshot(),afterAuthenticatedEdit);assert.equal(policySnapshot(),oldPolicies);assert.equal(aclSnapshot(),oldAcls);
  assert.equal(db("select count(*) from pg_policy where polrelid in ('public.polls'::regclass,'public.poll_questions'::regclass,'public.poll_responses'::regclass,'public.poll_answers'::regclass) and polname like '%anonymous_boundary' and not polpermissive and polcmd='*' and polroles=array[(select oid from pg_roles where rolname='anon')];"),'4');
  for(const table of tables)assert.equal(db("set role anon;set request.jwt.claims='{}';"+`select count(*) from public.${table};`),'0');
  db(role('authenticated',0)+`delete from public.polls where id in ('${created.id}','${newPoll.id}');`);for(const table of tables)assert.equal(db(`select count(*) from public.${table};`),'0');
 }finally{execFileSync('docker',['rm','-fv',name],{stdio:'pipe'})}
});
