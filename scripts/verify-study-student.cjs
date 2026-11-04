/** Contract fixtures only: no school HTTP requests, real users or school writes. */
const fs=require('node:fs'),assert=require('node:assert/strict'),ts=require('../../FrontEnd/cbgpu/node_modules/typescript');
require.extensions['.ts']=(m,file)=>m._compile(ts.transpileModule(fs.readFileSync(file,'utf8'),{compilerOptions:{module:ts.ModuleKind.CommonJS,target:ts.ScriptTarget.ES2022}}).outputText,file);
const {handleStudentStudy,studyWindow,StudyFailure}=require('../../FrontEnd/cbgpu/src/lib/edusetStudent.ts');
const token=id=>'fixture.'+Buffer.from(JSON.stringify({userId:id})).toString('base64url')+'.signature';
const schoolToken=token('student-user'),policy={timelimit:{assign:Array.from({length:7},()=>({start:{h:0,m:0},end:{h:24,m:0}})),exception:{}},timetable:{assign:Array(7).fill('day'),presets:{day:[{},{}]},exception:{}}};
const at=new Date('2026-11-03T07:00:00Z');
function fixture(){
 let records=[],next=10;const calls=[];let clock=at,admin=false,authorized=true,delay=null,uncertain=false;
 const rooms=[{id:31,name:'좌석실',hasSeats:true,width:2,height:1},{id:32,name:'정원실',hasSeats:false,width:1,height:1}];
 const teachers=[{id:'teacher-one',name:'담당교사'}];
 const send=async(path,t,method='GET',body)=>{
  calls.push({path,token:t,method,body:structuredClone(body)});
  if(path==='/auth/signin')return {accessToken:schoolToken};
  if(t!==schoolToken)throw new StudyFailure('Invalid school token',401);
  if(path==='/users/student-user')return {id:'student-user',role:admin?'ADMIN':'STUDENT'};
  if(path==='/students/student-user')return {id:'student-user',name:'학생',grade:2,classNo:1,number:1};
  if(path==='/policies/selfstudy')return {data:structuredClone(policy)};
  if(path==='/rooms')return rooms;
  if(path==='/teachers')return teachers;
  if(path==='/rooms/31/composition')return {seats:[{id:41,name:'1'}]};
  if(path==='/selfstudy/search')return structuredClone(records);
  if(path==='/selfstudy'&&method==='POST'){
   if(delay)await delay;
   const row={id:next++,date:body.date,type:'selfstudy',student:{id:'student-user'},period:body.period,status:'PENDING',room:rooms.find(r=>r.id===body.roomId),teacher:teachers[0],seat:body.seatId?{id:body.seatId,name:'1'}:null,reason:body.reason};records.push(row);
   if(uncertain)throw Error('connection lost after commit');return row;
  }
  if(path.startsWith('/selfstudy/')&&method==='DELETE'){records=records.filter(r=>r.id!==Number(path.split('/').pop()));return null}
  throw Error('Unexpected school request '+path);
 };
 const deps={authorize:async()=>authorized?'cbgpu-fixture':null,send,now:()=>clock,limit:()=>true,loginLimit:()=>true};
 const call=async(body,headers={})=>{const req=new Request('https://cbgpu.com/api/study/student',{method:body===undefined?'GET':'POST',headers:{host:'cbgpu.com',origin:'https://cbgpu.com','Content-Type':'application/json','x-eduset-token':schoolToken,...headers},body:body===undefined?undefined:JSON.stringify(body)});const r=await handleStudentStudy(req,deps);return {status:r.status,body:await r.json()}};
 return {calls,call,deps,set records(v){records=v},get records(){return records},set clock(v){clock=v},set admin(v){admin=v},set authorized(v){authorized=v},set delay(v){delay=v},set uncertain(v){uncertain=v}};
}
const apply={action:'apply',date:'2026-11-03',roomId:31,seatId:41,teacherId:'teacher-one',period:1,reason:'자율학습'};
(async()=>{
 let f=fixture();let r=await f.call();assert.equal(r.status,200);assert.equal(r.body.student.id,'student-user');assert.equal(r.body.open,true);assert.deepEqual(r.body.periods,[1,2]);
 f.authorized=false;r=await f.call(apply);assert.equal(r.status,401);assert.equal(f.calls.filter(c=>c.path==='/selfstudy').length,0);
 f=fixture();assert.equal((await f.call(apply,{origin:'https://evil.invalid'})).status,403);assert.equal(f.calls.length,0);
 f.admin=true;assert.equal((await f.call(apply)).status,403);assert.equal(f.calls.filter(c=>c.method==='POST').length,0);
 f=fixture();assert.equal((await f.call(apply,{'x-eduset-token':token(999)})).status,401);console.log('PASS CBGPU authentication, origin, verified school identity and admin-token rejection');
 f=fixture();r=await f.call({...apply,studentId:999,status:'APPROVED',attendedAt:'now',options:{overwrite:true}});assert.equal(r.status,200);const sent=f.calls.find(c=>c.path==='/selfstudy').body;assert.deepEqual(sent,{studentId:'student-user',date:'2026-11-03T00:00:00.000Z',roomId:31,teacherId:'teacher-one',period:1,reason:'자율학습',seatId:41});assert.equal(r.body.entries[0].status,'PENDING');
 assert.equal((await f.call(apply)).status,409);assert.equal(f.records.length,1);
 const other={...f.records[0],id:999,student:{id:22}};f.records.push(other);assert.equal((await f.call({action:'cancel',date:apply.date,id:999})).status,404);r=await f.call({action:'cancel',date:apply.date,id:10});assert.equal(r.status,200);assert.equal(r.body.entries.length,0);assert.ok(f.records.some(r=>r.id===999));console.log('PASS exact student request payload, duplicate prevention, confirmed apply/cancel and other-student isolation');
 for(const changes of [{date:'2026-11-04'},{period:3},{teacherId:999},{seatId:999},{roomId:32},{reason:''}]){f=fixture();assert.equal((await f.call({...apply,...changes})).status,400);assert.equal(f.calls.filter(c=>c.path==='/selfstudy').length,0)}
 f=fixture();r=await f.call({...apply,roomId:32,seatId:undefined});assert.equal(r.status,200);assert.ok(!('seatId' in f.calls.find(c=>c.path==='/selfstudy').body));console.log('PASS date/period/teacher/seat validation and capacity-room submissions');
 let p=structuredClone(policy);p.timelimit.exception['2026-11-03']={start:{h:9,m:0},end:{h:16,m:0}};assert.equal(studyWindow(p,new Date('2026-11-03T00:00:00Z')).open,true);assert.equal(studyWindow(p,at).open,false);assert.equal(studyWindow(policy,new Date('2026-11-03T14:59:59Z')).open,true);assert.equal(studyWindow({data:{}},at).open,false);
 f=fixture();f.clock=new Date('2026-11-03T15:00:00Z');assert.equal((await f.call(apply)).status,400); // KST midnight: wrong date rejected.
 f=fixture();const closedSend=f.deps.send;f.deps.send=async(...args)=>{if(args[0]==='/policies/selfstudy')return {data:p};return closedSend(...args)};assert.equal((await f.call(apply)).status,409);assert.equal(f.calls.filter(c=>c.path==='/selfstudy').length,0);console.log('PASS KST boundaries, daily exceptions and fail-closed unknown/closed policies');
 f=fixture();let resume;f.delay=new Promise(r=>resume=r);const first=f.call(apply);while(!f.calls.some(c=>c.path==='/selfstudy'))await new Promise(r=>setTimeout(r,1));assert.equal((await f.call(apply)).status,409);resume();assert.equal((await first).status,200);assert.equal(f.records.length,1);
 f=fixture();f.uncertain=true;assert.equal((await f.call(apply)).status,503);assert.equal((await f.call()).body.entries.length,1);assert.equal((await f.call(apply)).status,409);console.log('PASS concurrent requests and ambiguous-response reconciliation without retrying writes');
 f=fixture();r=await f.call({...apply,roomId:'31',seatId:'41'});assert.equal(r.status,200);assert.equal(f.calls.find(c=>c.path==='/selfstudy').body.roomId,31);assert.equal(f.calls.find(c=>c.path==='/selfstudy').body.seatId,41);
 f=fixture();assert.equal((await f.call(null)).status,400);assert.equal((await f.call({action:'connect',username:'x',password:'x',large:'x'.repeat(9000)})).status,413);assert.equal((await f.call({action:'connect',username:'x',password:'x'})).status,400);assert.ok(!f.calls.some(c=>c.path==='/auth/signin'));console.log('PASS real mixed ID types, bounded bodies and no password-login relay');
 f=fixture();const connections=new Map(),removed=[];
 f.deps.getConnection=async owner=>connections.get(owner)||null;
 f.deps.remember=async(owner,t,student)=>{assert.equal(student.id,'student-user');connections.set(owner,t)};
 f.deps.disconnect=async owner=>{removed.push(owner);connections.delete(owner)};
 const withoutHeader={'x-eduset-token':''};
 r=await f.call(undefined,withoutHeader);assert.equal(r.status,401);assert.equal(r.body.code,'school_connection_required');
 r=await f.call({action:'inspect',accessToken:schoolToken},withoutHeader);assert.equal(r.status,200);assert.equal(r.body.connected,false);assert.equal(connections.size,0);assert.ok(!('accessToken' in r.body));
 r=await f.call({action:'remember',accessToken:schoolToken},withoutHeader);assert.equal(r.status,200);assert.equal(connections.get('cbgpu-fixture'),schoolToken);assert.ok(!('accessToken' in r.body));
 assert.equal((await f.call(undefined,withoutHeader)).status,200);
 f.deps.authorize=async()=> 'other-owner';assert.equal((await f.call(undefined,withoutHeader)).status,401);assert.equal(connections.size,1);
 f.deps.authorize=async()=>null;assert.equal((await f.call({action:'disconnect'},withoutHeader)).status,401);assert.equal(connections.size,1);assert.equal(removed.length,0);
 f.deps.authorize=async()=> 'cbgpu-fixture';connections.set('cbgpu-fixture',token('expired-user'));r=await f.call(undefined,withoutHeader);assert.equal(r.status,401);assert.equal(r.body.code,'school_connection_required');assert.equal(connections.size,0);
 connections.set('cbgpu-fixture','expired-token');assert.equal((await f.call({action:'disconnect'},withoutHeader)).status,200);assert.equal(connections.size,0);
 assert.equal((await f.call({action:'remember',accessToken:token('other-user')},withoutHeader)).status,401);assert.equal(connections.size,0);
 console.log('PASS persistent school connection: inspect/confirm separation, no raw-token responses, owner isolation, revoked CBGPU denial, school expiry cleanup and expired-session disconnect');
 const {createStudyLinkHandler}=require('../../FrontEnd/cbgpu/src/lib/edusetStudyLink.ts');
 let owner='owner-a',clock=1000,verified=0;
 const link=createStudyLinkHandler({authorize:async()=>owner,limit:()=>true,now:()=>clock,verify:async t=>{verified++;if(t!=='fixture-school-token')throw new StudyFailure('Invalid school account',401);return {id:'student-user',name:'Fixture',grade:2,classNo:1,number:1}}});
 const handoff=async(body,origin='https://app.cbgpu.com')=>{const response=await link(new Request('https://app.cbgpu.com/api/study/link',{method:'POST',headers:{host:'app.cbgpu.com',origin,'Content-Type':'application/json'},body:JSON.stringify(body)}));return {status:response.status,body:await response.json()}};
 owner=null;assert.equal((await handoff({action:'start'})).status,401);owner='owner-a';
 const started=await handoff({action:'start'}),id=started.body.id;assert.match(id,/^[A-F0-9]{16}$/);assert.equal((await handoff({action:'poll',id})).body.ready,false);
 assert.equal((await handoff({action:'receive',id,accessToken:'fixture-school-token'},'https://evil.invalid')).status,403);assert.equal(verified,0);
 owner='owner-b';assert.equal((await handoff({action:'poll',id})).status,404);owner=null;
 assert.equal((await handoff({action:'receive',id,accessToken:'invalid'})).status,401);assert.equal((await handoff({action:'receive',id,accessToken:'fixture-school-token'})).status,200);
 assert.equal((await handoff({action:'receive',id,accessToken:'fixture-school-token'})).status,409);
 owner='owner-a';const ready=await handoff({action:'poll',id});assert.equal(ready.body.ready,true);assert.ok(!('accessToken' in ready.body));assert.equal((await handoff({action:'claim',id,studentId:'other-student'})).status,409);
 const claimed=await handoff({action:'claim',id,studentId:'student-user'});assert.equal(claimed.body.accessToken,'fixture-school-token');assert.equal((await handoff({action:'claim',id,studentId:'student-user'})).status,404);
 const expired=(await handoff({action:'start'})).body.id;clock+=300001;assert.equal((await handoff({action:'receive',id:expired,accessToken:'fixture-school-token'})).status,404);
 const cancelled=(await handoff({action:'start'})).body.id;assert.equal((await handoff({action:'cancel',id:cancelled})).status,200);assert.equal((await handoff({action:'poll',id:cancelled})).status,404);
 console.log('PASS school handoff: owner binding, origin, verified student session, no-token polling, explicit identity confirmation, single use, expiry and cancellation');
 console.log('All student-study contract checks passed. No real school API calls or records changed.');
})().catch(e=>{console.error(e);process.exitCode=1});
