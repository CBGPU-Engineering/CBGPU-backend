/** Real disposable PostgreSQL transactions. No production host, ports, records or credentials. */
import { execFileSync, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import fs from 'node:fs';
import assert from 'node:assert/strict';
import test from 'node:test';
const run = promisify(execFile);
const name = `cbgpu-poll-integrity-test-${process.pid}`;
const image = 'postgres:17-alpine';
const db = (sql: string) => execFileSync('docker', ['exec', '-i', name, 'psql', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1', '-U', 'postgres', '-d', 'postgres'], { input: sql, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] }).trim();
const claims = (admin = true) => `set role authenticated;set request.jwt.claims='${JSON.stringify({ sub: admin ? '11111111-1111-4111-8111-111111111111' : '22222222-2222-4222-8222-222222222222', session_id: admin ? 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa' : 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', role: 'authenticated' })}';`;
const literal = (value: unknown) => `'${JSON.stringify(value).replaceAll("'", "''")}'::jsonb`;
const question = (id: string, title = '질문', type = 'text', config = {}) => ({ id, type, title, description: '', required: false, options: [], config });
const save = (id: string | null, revision: number | null, questions: unknown[], title = '투표') => `select public.cbgpu_save_poll(${id ? `'${id}'` : 'null'},${revision ?? 'null'},${literal({ title, description: '설명', show_results: true })},${literal(questions)});`;
const state = (id: string) => JSON.parse(db(`select jsonb_build_object('poll',(select to_jsonb(p) from public.polls p where id='${id}'),'questions',(select jsonb_agg(to_jsonb(q) order by q.id) from public.poll_questions q where poll_id='${id}'),'answers',(select jsonb_agg(to_jsonb(a) order by a.id) from public.poll_answers a join public.poll_questions q on q.id=a.question_id where q.poll_id='${id}'));`));
const fails = (sql: string, message: string) => assert.throws(() => db(claims() + sql), error => String((error as { stderr?: string }).stderr).includes(message));
test('atomic poll editor preserves answers, rejects stale writes, and rolls back failures', async () => {
 execFileSync('docker', ['run', '--detach', '--name', name, '--network', 'none', '--env', 'POSTGRES_PASSWORD=synthetic-poll-test', image, 'postgres', '-c', 'listen_addresses='], { stdio: 'pipe' });
 try {
  for(let i=0;i<200;i++){const logs=execFileSync('docker',['logs',name],{encoding:'utf8',stdio:['ignore','pipe','pipe']});if(logs.includes('PostgreSQL init process complete; ready for start up.')){try{db('select 1');break}catch{}}await new Promise(r=>setTimeout(r,100));}
  db(fs.readFileSync(new URL('./bootstrap.sql',import.meta.url),'utf8'));
  db(fs.readFileSync(new URL('../../supabase/migrations/20261123162344_atomic_poll_editor_save.sql',import.meta.url),'utf8'));
  const created=JSON.parse(db(claims()+save(null,null,[question('short-a'),question('short-b','조건 질문','text',{showIf:{questionId:'short-a',value:'yes'}})])));
  const id=created.id;let snapshot=state(id);const first=snapshot.questions.find((q:any)=>q.order_num===0),second=snapshot.questions.find((q:any)=>q.order_num===1);
  assert.match(first.id,/^[0-9a-f-]{36}$/);assert.equal(second.config.showIf.questionId,first.id);
  db(`with r as(insert into public.poll_responses(poll_id,user_id) values('${id}','22222222-2222-4222-8222-222222222222') returning id) insert into public.poll_answers(response_id,question_id,value) select id,'${first.id}','"kept answer"'::jsonb from r;`);
  snapshot=state(id);const answer=snapshot.answers[0];
  let revision=snapshot.poll.editor_revision;
  db(claims()+save(id,revision,[question(second.id,'옮긴 질문','text',second.config),question(first.id,'바꾼 제목')],'변경된 투표'));
  snapshot=state(id);assert.deepEqual(snapshot.answers,[answer]);assert.equal(snapshot.questions.find((q:any)=>q.id===first.id).order_num,1);
  revision=snapshot.poll.editor_revision;
  fails(save(id,revision,[question(second.id)]),'POLL_ANSWERED_QUESTION_REMOVAL');assert.deepEqual(state(id),snapshot);
  fails(save(id,revision,[question(first.id,'유형 변경','scale'),question(second.id)]),'POLL_ANSWERED_QUESTION_TYPE');assert.deepEqual(state(id),snapshot);
  fails(save(id,revision,[question(first.id),question(first.id)]),'POLL_INVALID_INPUT');assert.deepEqual(state(id),snapshot);
  fails(save(id,revision,[question(first.id),question(second.id,'invalid dependency','text',{showIf:{questionId:'missing',value:'x'}})]),'POLL_INVALID_CONDITION');assert.deepEqual(state(id),snapshot);
  fails(`delete from public.poll_questions where poll_id='${id}';`,'POLL_ANSWERED_QUESTION_REMOVAL');assert.deepEqual(state(id),snapshot);
  fails(`update public.poll_questions set type='scale' where id='${first.id}';`,'POLL_ANSWERED_QUESTION_TYPE');assert.deepEqual(state(id),snapshot);
  assert.throws(()=>db("set role anon;"+save(id,revision,[])),e=>String((e as any).stderr).includes('permission denied'));
  assert.equal(db("select prosecdef from pg_proc where oid='public.cbgpu_save_poll(uuid,bigint,jsonb,jsonb)'::regprocedure"),'f');
  // A real database insertion failure occurs after prior header/question changes.
  db(`create function public.fail_synthetic_question() returns trigger language plpgsql as $$begin if new.title='FAIL_INSERT' then raise exception 'SYNTHETIC_INSERT_FAILURE';end if;return new;end$$;create trigger synthetic_insert_failure before insert on public.poll_questions for each row execute function public.fail_synthetic_question();`);
  fails(save(id,revision,[question(first.id,'would change'),question('new-fail','FAIL_INSERT')],'would change header'),'SYNTHETIC_INSERT_FAILURE');assert.deepEqual(state(id),snapshot);
  const count=db('select count(*) from public.polls');fails(save(null,null,[question('new-fail','FAIL_INSERT')]),'SYNTHETIC_INSERT_FAILURE');assert.equal(db('select count(*) from public.polls'),count);
  const foreign=JSON.parse(db(claims()+save(null,null,[question('foreign')]))).id;
  const foreignQuestion=state(foreign).questions[0].id;
  fails(save(id,revision,[question(first.id),question(foreignQuestion)]),'POLL_QUESTION_SCOPE');assert.deepEqual(state(id),snapshot);
  assert.throws(()=>db(claims(false)+save(id,revision,[question(first.id)])),e=>String((e as any).stderr).includes('POLL_ADMIN_REQUIRED'));
  db(`update public.profiles set account_status='SUSPENDED' where is_admin;`);fails(save(id,revision,[question(first.id)]),'POLL_ADMIN_REQUIRED');db(`update public.profiles set account_status='ACTIVE' where is_admin;`);
  db(`update auth.sessions set not_after=now()-interval '1 minute' where user_id='11111111-1111-4111-8111-111111111111';`);fails(save(id,revision,[question(first.id)]),'POLL_ADMIN_REQUIRED');db(`update auth.sessions set not_after=null;`);
  // Old direct clients also advance the editor revision.
  db(claims()+`update public.polls set title='legacy header edit' where id='${id}';`);
  fails(save(id,revision,[question(first.id),question(second.id)]),'POLL_EDIT_CONFLICT');
  revision=state(id).poll.editor_revision;
  db(claims()+`update public.poll_questions set title='legacy question edit' where id='${second.id}';`);
  fails(save(id,revision,[question(first.id),question(second.id)]),'POLL_EDIT_CONFLICT');
  revision=state(id).poll.editor_revision;
  const pendingSql="set application_name='atomic-poll-winner';begin;"+claims()+save(id,revision,[question(first.id),question(second.id)],'concurrent winner')+'select pg_sleep(1);commit;';
  const pending=run('docker',['exec','-i',name,'psql','-X','-qAt','-v','ON_ERROR_STOP=1','-U','postgres','-d','postgres','-c',pendingSql]);
  for(let i=0;i<100;i++){if(db("select count(*) from pg_stat_activity where application_name='atomic-poll-winner' and wait_event='PgSleep'")==='1')break;await new Promise(r=>setTimeout(r,20));}
  fails(save(id,revision,[question(first.id),question(second.id)],'concurrent stale'),'POLL_EDIT_CONFLICT');await pending;
  assert.equal(state(id).poll.title,'concurrent winner');assert.deepEqual(state(id).answers,[answer]);
  // Choice IDs are persisted answer identities, not disposable editor labels.
  const options=[{id:'kept',text:'유지할 선택지'},{id:'other',text:'다른 선택지'}];
  const pick=(client:string,type='choice',opts=options)=>({...question(client,'선택 질문',type),options:opts});
  const choices=JSON.parse(db(claims()+save(null,null,[pick('choice'),pick('checks','checkbox'),question('dependent','조건 질문','text',{showIf:{questionId:'choice',value:'kept'}})]))).id;
  let choiceState=state(choices);const choiceQ=choiceState.questions.find((q:any)=>q.order_num===0),checkQ=choiceState.questions.find((q:any)=>q.order_num===1);
  assert.equal(choiceState.questions.find((q:any)=>q.order_num===2).config.showIf.questionId,choiceQ.id);
  const choiceResponse=db(claims(false)+`insert into public.poll_responses(poll_id,user_id) values('${choices}','22222222-2222-4222-8222-222222222222') returning id;`);
  db(claims(false)+`insert into public.poll_answers(response_id,question_id,value) values('${choiceResponse}','${choiceQ.id}','"kept"'::jsonb),('${choiceResponse}','${checkQ.id}','["kept","other"]'::jsonb);`);
  choiceState=state(choices);const chosenSnapshot=choiceState;
  const changed=choiceState.questions.map((q:any)=>q.id===checkQ.id?{...q,options:[options[1]]}:q);
  fails(save(choices,choiceState.poll.editor_revision,changed),'POLL_ANSWERED_OPTION_REMOVAL');assert.deepEqual(state(choices),chosenSnapshot);
  fails(`update public.poll_questions set options='[]'::jsonb where id='${checkQ.id}';`,'POLL_ANSWERED_OPTION_REMOVAL');assert.deepEqual(state(choices),chosenSnapshot);
  const duplicates=choiceState.questions.map((q:any)=>q.id===choiceQ.id?{...q,options:[options[0],options[0]]}:q);
  fails(save(choices,choiceState.poll.editor_revision,duplicates),'POLL_INVALID_OPTIONS');assert.deepEqual(state(choices),chosenSnapshot);
  const alias={...choiceQ,id:choiceQ.id.toUpperCase()};
  if(alias.id===choiceQ.id)alias.id=choiceQ.id.replace(/[a-f]/,char=>char.toUpperCase());
  fails(save(choices,choiceState.poll.editor_revision,[...choiceState.questions,alias]),'POLL_INVALID_INPUT');assert.deepEqual(state(choices),chosenSnapshot);
  const invalidCondition=choiceState.questions.map((q:any)=>q.order_num===2?{...q,config:{showIf:{questionId:choiceQ.id,value:'missing'}}}:q);
  fails(save(choices,choiceState.poll.editor_revision,invalidCondition),'POLL_INVALID_CONDITION');assert.deepEqual(state(choices),chosenSnapshot);
  const countBeforeCycle=db('select count(*) from public.polls');
  fails(save(null,null,[question('cycle-a','A','text',{showIf:{questionId:'cycle-b',value:'yes'}}),question('cycle-b','B','text',{showIf:{questionId:'cycle-a',value:'yes'}})]),'POLL_INVALID_CONDITION');assert.equal(db('select count(*) from public.polls'),countBeforeCycle);
  // Reordering remains valid even when a conditional question precedes its source.
  db(claims()+save(choices,choiceState.poll.editor_revision,[...choiceState.questions].reverse()));
  assert.deepEqual(state(choices).answers,chosenSnapshot.answers);
  // Legacy non-key updates must wait for a concurrent answer's FK lock, then
  // re-check the freshly committed answer before changing its interpretation.
  const racing=JSON.parse(db(claims()+save(null,null,[question('race-q')]))).id;
  const racingQ=state(racing).questions[0].id;
  const answering=run('docker',['exec','-i',name,'psql','-X','-qAt','-v','ON_ERROR_STOP=1','-U','postgres','-d','postgres','-c',"set application_name='poll-answer-before-type';begin;"+claims(false)+`insert into public.poll_responses(poll_id,user_id) values('${racing}','22222222-2222-4222-8222-222222222222');insert into public.poll_answers(response_id,question_id,value) select id,'${racingQ}','"text answer"'::jsonb from public.poll_responses where poll_id='${racing}' and user_id=auth.uid();select pg_sleep(1);commit;`]);
  let answerPending=false;for(let i=0;i<100;i++){if(db("select count(*) from pg_stat_activity where application_name='poll-answer-before-type' and wait_event='PgSleep'")==='1'){answerPending=true;break}await new Promise(r=>setTimeout(r,20));}
  assert.equal(answerPending,true);
  fails(`update public.poll_questions set type='scale' where id='${racingQ}';`,'POLL_ANSWERED_QUESTION_TYPE');await answering;
  assert.equal(state(racing).questions[0].type,'text');assert.equal(state(racing).answers[0].value,'text answer');
  // Explicit whole-poll deletion remains available through the original RLS.
  db(claims()+`delete from public.polls where id='${id}';`);
  assert.equal(db(`select count(*) from public.poll_answers where id='${answer.id}'`),'0');
  assert.equal(db(`select count(*) from public.poll_questions where poll_id='${id}'`),'0');
 } catch(error) { console.error(execFileSync('docker',['logs',name],{encoding:'utf8',stdio:['ignore','pipe','pipe']}).slice(-2000));throw error; } finally { execFileSync('docker',['rm','-fv',name],{stdio:'pipe'}); }
});
