// Private DB-trigger receiver: notify only currently eligible equipment managers.
import {createClient} from 'npm:@supabase/supabase-js@2.112.4';
import {corsHeaders,handleOptions} from '../_shared/cors.ts';
import {getFcmAccessToken,sendFcmPush} from '../_shared/fcm.ts';
import {requireServiceRole} from '../_shared/internalAuth.ts';
Deno.serve(async(req)=>{
 const preflight=handleOptions(req);if(preflight)return preflight;
 if(!requireServiceRole(req))return Response.json({message:'forbidden'},{status:401,headers:corsHeaders});
 if(req.method!=='POST')return Response.json({message:'method not allowed'},{status:405,headers:corsHeaders});
 const {type,job_id,dry_run=false}=await req.json().catch(()=>({}));
 if(!['print','laser'].includes(type)||typeof job_id!=='string'||!/^[a-f0-9-]{36}$/i.test(job_id))return Response.json({message:'invalid payload'},{status:400,headers:corsHeaders});
 const admin=createClient(Deno.env.get('SUPABASE_URL')!,Deno.env.get('SUPABASE_SERVICE_ROLE_KEY')!);
 const table=type==='print'?'print_jobs':'laser_jobs';
 // user_id references auth.users; PostgREST cannot embed requester:user_id from public.profiles.
 const {data:job,error:jobError}=await admin.from(table).select('*').eq('id',job_id).maybeSingle();
 if(jobError)return Response.json({message:'Job lookup failed'},{status:500,headers:corsHeaders});
 if(!job)return Response.json({message:'job not found'},{status:404,headers:corsHeaders});
 let pool:string[]=[];
 if(type==='print'){
  let query=admin.from('printer_managers').select('user_id');const printer=job.preferred_printer_id??job.printer_id;
  if(printer!==null&&printer!==undefined)query=query.eq('printer_id',printer);
  const {data,error}=await query;if(error)return Response.json({message:'Printer manager lookup failed'},{status:500,headers:corsHeaders});
  pool=(data??[]).map((row:{user_id:string})=>row.user_id);
 }else{
  const {data,error}=await admin.from('profiles').select('id').eq('is_laser_cut_manager',true);
  if(error)return Response.json({message:'Laser manager lookup failed'},{status:500,headers:corsHeaders});pool=(data??[]).map((row:{id:string})=>row.id);
 }
 pool=[...new Set(pool)];if(job.manager_id)pool=pool.filter(id=>id===job.manager_id);
 if(!pool.length)return Response.json({ok:true,dry_run,notified:0,eligible_managers:0,registered_devices:0,reason:'no eligible managers'},{headers:corsHeaders});
 const {data:profiles,error:profileError}=await admin.from('profiles').select('id,is_approved').in('id',pool);
 if(profileError)return Response.json({message:'Manager account lookup failed'},{status:500,headers:corsHeaders});
 const targets=(profiles??[]).filter(p=>p.is_approved!==false).map(p=>p.id);
 if(!targets.length)return Response.json({ok:true,dry_run,notified:0,eligible_managers:0,registered_devices:0,reason:'no approved managers'},{headers:corsHeaders});
 const {data:rows,error:tokenError}=await admin.from('device_tokens').select('token').in('user_id',targets).eq('platform','android');
 if(tokenError)return Response.json({message:'Device lookup failed'},{status:500,headers:corsHeaders});
 const tokens=[...new Set((rows??[]).map(row=>row.token).filter(Boolean))];
 // Private diagnostics resolve recipient counts without sending messages.
 if(dry_run===true)return Response.json({ok:true,dry_run:true,eligible_managers:targets.length,registered_devices:tokens.length,notification_type:type},{headers:corsHeaders});
 if(!tokens.length)return Response.json({ok:true,notified:0,reason:'no device tokens'},{headers:corsHeaders});
 const raw=Deno.env.get('FIREBASE_SERVICE_ACCOUNT_JSON');if(!raw)return Response.json({message:'Push credentials are not configured'},{status:503,headers:corsHeaders});
 try{
  const serviceAccount=JSON.parse(raw),accessToken=await getFcmAccessToken(serviceAccount);
  const {data:requester}=await admin.from('profiles').select('full_name,student_id').eq('id',job.user_id).maybeSingle();
  const label=requester?.full_name?`${requester.full_name}(${requester.student_id??'-'})`:'누군가';
  const title=type==='print'?'새 3D프린팅 신청':'새 레이저 커팅 신청';
  const body=type==='print'?`${label}님이 "${job.file_name??'파일'}" 인쇄를 신청했습니다.`:`${label}님이 ${job.material==='mdf'?'MDF':'아크릴'} ${job.thickness_mm??''}mm 커팅을 신청했습니다.`;
  let notified=0,failed=0;for(const token of tokens){try{await sendFcmPush(serviceAccount.project_id,accessToken,token,title,body,'cbgpu_requests');notified++}catch{failed++}}
  return Response.json({ok:failed===0,notified,failed},{status:failed&&notified===0?502:200,headers:corsHeaders});
 }catch{return Response.json({message:'Push service authentication failed'},{status:502,headers:corsHeaders})}
});
