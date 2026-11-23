/** Real PostgreSQL + existing-role RLS; only an owned network-isolated disposable container. */
import {execFileSync,execFile} from 'node:child_process';import {promisify} from 'node:util';import fs from 'node:fs';import assert from 'node:assert/strict';import test from 'node:test';import {randomUUID} from 'node:crypto';
const run=promisify(execFile),name=`cbgpu-poll-response-test-${process.pid}`;
const db=(sql:string)=>execFileSync('docker',['exec','-i',name,'psql','-X','-qAt','-v','ON_ERROR_STOP=1','-U','postgres','-d','postgres'],{input:sql,encoding:'utf8',stdio:['pipe','pipe','pipe']}).trim();
const owner='22222222-2222-4222-8222-222222222222';const claims=`set role authenticated;set request.jwt.claims='${JSON.stringify({sub:owner,session_id:'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',role:'authenticated'})}';`;
const json=(value:unknown)=>`'${JSON.stringify(value).replaceAll("'","''")}'::jsonb`;
const submit=(poll:any,answers:unknown)=>`select public.cbgpu_submit_poll('${poll.id}',${poll.revision},${json(answers)});`;
const failure=(sql:string,message:string)=>assert.throws(()=>db(claims+sql),e=>String((e as any).stderr).includes(message));
const make=(questions:any[],settings:any={})=>{const id=randomUUID();db(`insert into public.polls(id,title,is_active,ends_at) values('${id}','합성 투표',${settings.open===false?'false':'true'},${settings.expired?"now()-interval '1 minute'":'null'});`);for(const q of questions)db(`insert into public.poll_questions(id,poll_id,type,title,required,options,config) values('${q.id}','${id}','${q.type||'text'}','질문',${!!q.required},${json(q.options||[])},${json(q.config||{})});`);return{id,revision:Number(db(`select editor_revision from public.polls where id='${id}'`))}};
const count=(poll:any)=>db(`select count(*) from public.poll_responses where poll_id='${poll.id}'`);
const answers=(id:string)=>JSON.parse(db(`select coalesce(jsonb_agg(to_jsonb(a) order by a.question_id),'[]') from public.poll_answers a where response_id='${id}'`));
const rawHeader=(poll:any,user=owner)=>db(`insert into public.poll_responses(poll_id,user_id) values('${poll.id}','${user}') returning id`);
const start=(sql:string)=>run('docker',['exec','-i',name,'psql','-X','-qAt','-v','ON_ERROR_STOP=1','-U','postgres','-d','postgres','-c',sql]);
const sleeping=async(app:string)=>{for(let i=0;i<100;i++){if(db(`select count(*) from pg_stat_activity where application_name='${app}' and wait_event='PgSleep'`)==='1')return;await new Promise(r=>setTimeout(r,20))}throw Error('Fixture lock holder did not become ready')};
test('atomic response submission validates, recovers owned empty headers, and coordinates legacy writers',async()=>{
 execFileSync('docker',['run','--detach','--name',name,'--network','none','--env','POSTGRES_PASSWORD=synthetic-poll-test','postgres:17-alpine','postgres','-c','listen_addresses='],{stdio:'pipe'});
 try{
  for(let i=0;i<200;i++){const logs=execFileSync('docker',['logs',name],{encoding:'utf8',stdio:['ignore','pipe','pipe']});if(logs.includes('PostgreSQL init process complete; ready for start up.')){try{db('select 1');break}catch{}}await new Promise(r=>setTimeout(r,100))}
  db(fs.readFileSync(new URL('./bootstrap.sql',import.meta.url),'utf8'));
  db(fs.readFileSync(new URL('../../supabase/migrations/20261123162344_atomic_poll_editor_save.sql',import.meta.url),'utf8'));
  db(fs.readFileSync(new URL('../../supabase/migrations/20261123164337_atomic_poll_response_submit.sql',import.meta.url),'utf8'));
  const a=randomUUID(),b=randomUUID(),c=randomUUID(),scale=randomUUID();
  const poll=make([{id:a,required:true},{id:b,type:'checkbox',options:[{id:'x',text:'X'},{id:'y',text:'Y'}]},{id:c,required:true,config:{showIf:{questionId:b,value:'y'}}},{id:scale,type:'scale',config:{min:1,max:3}}]);
  failure(submit(poll,{}),'POLL_REQUIRED_ANSWER');assert.equal(count(poll),'0');
  failure(submit(poll,{[a]:'yes',[b]:['y']}),'POLL_REQUIRED_ANSWER');
  failure(submit(poll,{[a]:'yes',[b]:['foreign']}),'POLL_INVALID_ANSWER');
  failure(submit(poll,{[a]:'yes',[scale]:4}),'POLL_INVALID_ANSWER');
  failure(submit(poll,{[a]:'yes',[scale]:[]}),'POLL_INVALID_ANSWER');
  failure(submit(poll,{[a]:'yes',[randomUUID()]:'foreign'}),'POLL_INVALID_ANSWER');assert.equal(count(poll),'0');
  const payload={[a]:'preserved',[b]:['x'],[scale]:2};const first=JSON.parse(db(claims+submit(poll,payload)));const saved=answers(first.id);assert.equal(saved.length,3);assert.equal(saved.some((row:any)=>row.question_id===c),false);
  const retry=JSON.parse(db(claims+submit(poll,payload)));assert.equal(retry.id,first.id);assert.equal(retry.reused,true);assert.deepEqual(answers(first.id),saved);assert.equal(count(poll),'1');
  failure(submit(poll,{...payload,[a]:'overwrite'}),'POLL_RESPONSE_ALREADY_RECORDED');assert.deepEqual(answers(first.id),saved);
  failure(submit({...poll,revision:poll.revision-1},payload),'POLL_EDIT_CONFLICT');
  const checkId=randomUUID(),checkPoll=make([{id:checkId,type:'checkbox',options:[{id:'x',text:'X'},{id:'y',text:'Y'}]}]),checkHeader=rawHeader(checkPoll);
  db(claims+`insert into public.poll_answers(response_id,question_id,value) values('${checkHeader}','${checkId}','["y","x"]');`);
  const checkboxBefore=answers(checkHeader);const sameSet=JSON.parse(db(claims+submit(checkPoll,{[checkId]:['x','y','x']})));assert.equal(sameSet.id,checkHeader);assert.equal(sameSet.reused,true);assert.deepEqual(answers(checkHeader),checkboxBefore);
  const zero=make([]),zeroFirst=JSON.parse(db(claims+submit(zero,{}))),zeroRetry=JSON.parse(db(claims+submit(zero,{})));assert.equal(zeroFirst.id,zeroRetry.id);assert.equal(zeroRetry.reused,true);assert.equal(answers(zeroFirst.id).length,0);
  const hidden=make([{id:randomUUID(),required:true,config:{showIf:{questionId:randomUUID(),value:'not-selected'}}}]);assert.equal(answers(JSON.parse(db(claims+submit(hidden,{}))).id).length,0);
  const required=randomUUID(),emptyPoll=make([{id:required,required:true}]);const emptyId=rawHeader(emptyPoll),foreignId=rawHeader(emptyPoll,'11111111-1111-4111-8111-111111111111');const resumed=JSON.parse(db(claims+submit(emptyPoll,{[required]:'resumed'})));assert.equal(resumed.id,emptyId);assert.equal(answers(emptyId).length,1);assert.equal(answers(foreignId).length,0);assert.equal(count(emptyPoll),'2');
  // A late database INSERT failure rolls back both a new header and earlier answers.
  db(`create function public.synthetic_answer_failure() returns trigger language plpgsql as $$begin if new.value='"FAIL_INSERT"'::jsonb then raise exception 'SYNTHETIC_ANSWER_FAILURE';end if;return new;end$$;create trigger synthetic_answer_failure before insert on public.poll_answers for each row execute function public.synthetic_answer_failure();`);
  const failA=randomUUID(),failB=randomUUID(),failPoll=make([{id:failA},{id:failB}]);failure(submit(failPoll,{[failA]:'kept draft',[failB]:'FAIL_INSERT'}),'SYNTHETIC_ANSWER_FAILURE');assert.equal(count(failPoll),'0');
  const retained=rawHeader(failPoll);failure(submit(failPoll,{[failA]:'kept draft',[failB]:'FAIL_INSERT'}),'SYNTHETIC_ANSWER_FAILURE');assert.equal(count(failPoll),'1');assert.deepEqual(answers(retained),[]);assert.equal(JSON.parse(db(claims+submit(failPoll,{[failA]:'valid',[failB]:'valid'}))).id,retained);
  for(const settings of [{open:false},{expired:true}]){const closed=make([],settings);failure(submit(closed,{}),'POLL_CLOSED');assert.equal(count(closed),'0')}
  // Old authenticated tabs cannot append answers after closure/deadline;
  // the same valid historical import remains available to maintenance roles.
  db('create role service_role bypassrls;grant all on public.polls,public.poll_questions,public.poll_responses,public.poll_answers to service_role;');
  for(const settings of [{open:false},{expired:true}]){
   const qid=randomUUID(),legacyClosed=make([{id:qid}],settings),header=rawHeader(legacyClosed);
   failure(`insert into public.poll_answers(response_id,question_id,value) values('${header}','${qid}','"too late"');`,'POLL_CLOSED');assert.deepEqual(answers(header),[]);
   db(`set role service_role;insert into public.poll_answers(response_id,question_id,value) values('${header}','${qid}','"historical import"');`);assert.equal(answers(header)[0].value,'historical import');
  }
  db(`update public.profiles set account_status='SUSPENDED' where id='${owner}'`);failure(submit(poll,payload),'POLL_AUTH_REQUIRED');db(`update public.profiles set account_status='ACTIVE' where id='${owner}'`);
  db(`update auth.sessions set not_after=now()-interval '1 minute' where user_id='${owner}'`);failure(submit(poll,payload),'POLL_AUTH_REQUIRED');db(`update auth.sessions set not_after=null`);
  assert.throws(()=>db('set role anon;'+submit(poll,payload)),e=>String((e as any).stderr).includes('permission denied'));
  assert.equal(db("select prosecdef from pg_proc where oid='public.cbgpu_submit_poll(uuid,bigint,jsonb)'::regprocedure"),'f');
  // Two current clients serialize to one response/answer set.
  const cq=randomUUID(),concurrent=make([{id:cq}]);const winner=start(`set application_name='poll-rpc-winner';begin;${claims}${submit(concurrent,{[cq]:'same'})}select pg_sleep(1);commit;`);await sleeping('poll-rpc-winner');const same=JSON.parse(db(claims+submit(concurrent,{[cq]:'same'})));await winner;assert.equal(same.reused,true);assert.equal(count(concurrent),'1');assert.equal(answers(same.id).length,1);
  // A legacy INSERT that wins is never overwritten by an RPC repairing that header.
  const lq=randomUUID(),legacy=make([{id:lq}]),legacyId=rawHeader(legacy);const oldFirst=start(`set application_name='poll-legacy-winner';begin;${claims}insert into public.poll_answers(response_id,question_id,value) values('${legacyId}','${lq}','"legacy wins"');select pg_sleep(1);commit;`);await sleeping('poll-legacy-winner');failure(submit(legacy,{[lq]:'different'}),'POLL_RESPONSE_ALREADY_RECORDED');await oldFirst;assert.equal(answers(legacyId)[0].value,'legacy wins');
  // When the RPC wins, a blocked old-tab INSERT sees the committed answer and fails.
  const rq=randomUUID(),rpcPoll=make([{id:rq}]),rpcId=rawHeader(rpcPoll);const rpcFirst=start(`set application_name='poll-rpc-before-legacy';begin;${claims}${submit(rpcPoll,{[rq]:'rpc wins'})}select pg_sleep(1);commit;`);await sleeping('poll-rpc-before-legacy');failure(`insert into public.poll_answers(response_id,question_id,value) values('${rpcId}','${rq}','"legacy duplicate"');`,'POLL_ANSWER_ALREADY_RECORDED');await rpcFirst;assert.equal(answers(rpcId).length,1);assert.equal(answers(rpcId)[0].value,'rpc wins');
  // Editor commits first: a stale legacy INSERT waits, then validates the new
  // type/options rather than attaching an old-shaped value after the update.
  const adminClaims=`set role authenticated;set request.jwt.claims='${JSON.stringify({sub:'11111111-1111-4111-8111-111111111111',session_id:'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',role:'authenticated'})}';`;
  for(const kind of ['type','choice','checkbox']){
   const questionId=randomUUID(),reverse=make([{id:questionId,type:kind==='type'?'text':kind,options:kind==='type'?[]:[{id:'old',text:'Old'},{id:'new',text:'New'}]}]),header=rawHeader(reverse);
   const change=kind==='type'?"type='scale'":`options='[{"id":"new","text":"New"}]'::jsonb`;
   const app='poll-editor-first-'+kind;
   const editor=start(`set application_name='${app}';begin;${adminClaims}update public.poll_questions set ${change} where id='${questionId}';select pg_sleep(1);commit;`);await sleeping(app);
   const oldValue=kind==='type'?'stale old text':kind==='checkbox'?['old']:'old';
   failure(`insert into public.poll_answers(response_id,question_id,value) values('${header}','${questionId}',${json(oldValue)});`,'POLL_INVALID_ANSWER');await editor;assert.deepEqual(answers(header),[]);
  }
  // Answer commits first: the legacy type change then sees it and rolls back.
  const aq=randomUUID(),answerFirstPoll=make([{id:aq}]),answerFirstHeader=rawHeader(answerFirstPoll);
  const activeAnswer=start(`set application_name='poll-answer-before-editor';begin;${claims}insert into public.poll_answers(response_id,question_id,value) values('${answerFirstHeader}','${aq}','"kept text"');select pg_sleep(1);commit;`);await sleeping('poll-answer-before-editor');
  assert.throws(()=>db(adminClaims+`update public.poll_questions set type='scale' where id='${aq}';`),e=>String((e as any).stderr).includes('POLL_ANSWERED_QUESTION_TYPE'));await activeAnswer;assert.equal(answers(answerFirstHeader)[0].value,'kept text');
  const duplicatePoll=make([{id:randomUUID()}]),dupId=rawHeader(duplicatePoll),dupQ=db(`select id from public.poll_questions where poll_id='${duplicatePoll.id}'`);failure(`insert into public.poll_answers(response_id,question_id,value) values('${dupId}','${dupQ}','"first"'),('${dupId}','${dupQ}','"second"');`,'POLL_ANSWER_ALREADY_RECORDED');assert.deepEqual(answers(dupId),[]);
  failure(`insert into public.poll_answers(response_id,question_id,value) values('${dupId}','${a}','"cross poll"');`,'POLL_RESPONSE_QUESTION_SCOPE');assert.deepEqual(answers(dupId),[]);
 }finally{execFileSync('docker',['rm','-fv',name],{stdio:'pipe'})}
});
