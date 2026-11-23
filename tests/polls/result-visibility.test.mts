/** Actual RLS, synthetic fresh sessions; never connects to an existing database. */
import test from 'node:test';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import fs from 'node:fs';
const name=`cbgpu-poll-results-test-${process.pid}`;
const db=(sql:string)=>execFileSync('docker',['exec','-i',name,'psql','-X','-qAt','-v','ON_ERROR_STOP=1','-U','postgres','-d','postgres'],{input:sql,encoding:'utf8',stdio:['pipe','pipe','pipe']}).trim();
const actors=['11111111-1111-4111-8111-111111111111','22222222-2222-4222-8222-222222222222','33333333-3333-4333-8333-333333333333'];
const sessions=['aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa','bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb','cccccccc-cccc-4ccc-8ccc-cccccccccccc'];
const actor=(i:number)=>`set role authenticated;set request.jwt.claims='${JSON.stringify({sub:actors[i],session_id:sessions[i],role:'authenticated'})}';`;
const literal=(value:unknown)=>`'${JSON.stringify(value).replaceAll("'","''")}'::jsonb`;
test('hidden poll answers are owner/admin only and visibility changes never mutate data',async()=>{
 execFileSync('docker',['run','--detach','--name',name,'--network','none','--env','POSTGRES_PASSWORD=synthetic-result-test','postgres:17-alpine','postgres','-c','listen_addresses='],{stdio:'pipe'});
 try{
  let ready=false;for(let i=0;i<200;i++){const logs=execFileSync('docker',['logs',name],{encoding:'utf8',stdio:['ignore','pipe','pipe']});if(logs.includes('PostgreSQL init process complete; ready for start up.')){try{db('select 1');ready=true;break}catch{}}await new Promise(r=>setTimeout(r,100))}assert.equal(ready,true);
  db(fs.readFileSync(new URL('./bootstrap.sql',import.meta.url),'utf8'));
  for(const migration of ['20261123162344_atomic_poll_editor_save.sql','20261123164337_atomic_poll_response_submit.sql','20261123165638_poll_result_visibility.sql','20261123175207_deny_anonymous_internal_polls.sql'])db(fs.readFileSync(new URL('../../supabase/migrations/'+migration,import.meta.url),'utf8'));
  db(`insert into auth.users(id)values('${actors[2]}');insert into public.profiles(id)values('${actors[2]}');insert into auth.sessions(id,user_id)values('${sessions[2]}','${actors[2]}');`);
  const saved=JSON.parse(db(actor(0)+`select public.cbgpu_save_poll(null,null,${literal({title:'Private result test',show_results:false})},${literal([{id:'question',title:'Answer',type:'text',options:[],config:{},required:true}])});`));
  const q=db(`select id from public.poll_questions where poll_id='${saved.id}';`),submit=(i:number,value:string)=>db(actor(i)+`select public.cbgpu_submit_poll('${saved.id}',${saved.editor_revision},${literal({[q]:value})});`);
  submit(1,'Private owner two');
  assert.equal(db(actor(2)+'select count(*) from public.poll_answers;'),'0');
  assert.equal(db(actor(2)+'select count(*) from public.poll_responses;'),'0');
  assert.equal(db(actor(1)+'select count(*) from public.poll_answers;'),'1');
  assert.equal(db(actor(0)+'select count(*) from public.poll_answers;'),'1');
  submit(2,'Private owner three');
  for(const i of[1,2])assert.equal(db(actor(i)+'select count(*) from public.poll_answers;'),'1');
  for(const i of[1,2])assert.equal(db(actor(i)+'select count(*) from public.poll_responses;'),'1');
  assert.equal(db(actor(0)+'select count(*) from public.poll_responses;'),'2');
  assert.equal(db(actor(0)+'select count(*) from public.poll_answers;'),'2');
  const values=db('select jsonb_agg(to_jsonb(a) order by id) from public.poll_answers a;');
  db(actor(0)+`update public.polls set show_results=true where id='${saved.id}';`);
  assert.equal(db(actor(1)+'select count(*) from public.poll_answers;'),'2');
  assert.equal(db(actor(1)+'select count(*) from public.poll_responses;'),'2');
  db(actor(0)+`update public.polls set show_results=false where id='${saved.id}';`);
  assert.equal(db(actor(1)+'select count(*) from public.poll_answers;'),'1');
  db(`update public.profiles set account_status='SUSPENDED' where id='${actors[1]}';`);
  assert.equal(db(actor(1)+'select count(*) from public.poll_answers;'),'0');
  assert.equal(db(actor(1)+'select count(*) from public.polls;'),'0');
  assert.equal(db(actor(1)+'select count(*) from public.poll_responses;'),'0');
  assert.throws(()=>db(actor(1)+`insert into public.poll_responses(poll_id,user_id)values('${saved.id}','${actors[1]}');`));
  assert.equal(db(actor(2)+'select count(*) from public.poll_answers;'),'1');
  db(`update auth.sessions set not_after=now()-interval '1 minute' where id='${sessions[2]}';`);
  assert.equal(db(actor(2)+'select count(*) from public.poll_answers;'),'0');
  assert.equal(db(actor(2)+'select count(*) from public.poll_questions;'),'0');
  for(const table of ['polls','poll_questions','poll_responses','poll_answers'])assert.equal(db('set role anon;select count(*) from public.'+table+';'),'0');
  for(let i=0;i<2;i++)db(fs.readFileSync(new URL('../../supabase/migrations/20261123165638_poll_result_visibility.sql',import.meta.url),'utf8'));
  assert.equal(db('select jsonb_agg(to_jsonb(a) order by id) from public.poll_answers a;'),values);
  assert.equal(db("select polpermissive from pg_policy where polname='cbgpu_poll_answer_result_visibility' and polrelid='public.poll_answers'::regclass;"),'f');
  db(actor(0)+`delete from public.polls where id='${saved.id}';`);
  assert.equal(db('select count(*) from public.poll_answers;'),'0');
  assert.equal(db('select count(*) from public.poll_questions;'),'0');
  // Optional blank selections are not option IDs and must not freeze editing.
  db(`update public.profiles set account_status='ACTIVE' where id='${actors[1]}';update auth.sessions set not_after=null where id='${sessions[1]}';`);
  const blankQuestion={id:'optional-choice',title:'Optional choice',type:'choice',options:[{id:'kept',text:'Before'}],config:{},required:false};
  const blankPoll=JSON.parse(db(actor(0)+`select public.cbgpu_save_poll(null,null,${literal({title:'Optional blank',show_results:false})},${literal([blankQuestion])});`));
  const blankId=db(`select id from public.poll_questions where poll_id='${blankPoll.id}';`);
  const blankResponse=JSON.parse(db(actor(1)+`select public.cbgpu_submit_poll('${blankPoll.id}',${blankPoll.editor_revision},${literal({[blankId]:''})});`));
  assert.equal(blankResponse.submitted,true);
  db(actor(0)+`select public.cbgpu_save_poll('${blankPoll.id}',${blankPoll.editor_revision},${literal({title:'Optional blank',show_results:false})},${literal([{...blankQuestion,id:blankId,options:[{id:'kept',text:'After'},{id:'new',text:'Added'}]}])});`);
  assert.equal(db(`select value from public.poll_answers where response_id='${blankResponse.id}';`),'""');
  assert.equal(db(`select options->0->>'text' from public.poll_questions where id='${blankId}';`),'After');

 }finally{execFileSync('docker',['rm','-fv',name],{stdio:'pipe'})}
});
