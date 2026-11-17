const json=(v,status=200)=>new Response(JSON.stringify(v),{status,headers:{'Content-Type':'application/json','Cache-Control':'no-store'}});
const keyOK=async(a,b)=>{const enc=new TextEncoder(),[x,y]=await Promise.all([crypto.subtle.digest('SHA-256',enc.encode(a||'')),crypto.subtle.digest('SHA-256',enc.encode(b||''))]);return new Uint8Array(x).every((v,i)=>v===new Uint8Array(y)[i])};
async function probe(url,headers={}){try{const r=await fetch(url,{headers:{'User-Agent':'CBGPU-Operations/1.0',...headers},signal:AbortSignal.timeout(12000),redirect:'manual'});const ok=r.ok;await r.body?.cancel();return ok}catch{return false}}
async function check(env,send=true){
 const checks=Object.fromEntries(await Promise.all([['website','https://cbgpu.com/'],['developers','https://developers.cbgpu.com/health'],['backend','https://developers.cbgpu.com/health/ready'],['admin','https://admin.cbgpu.com/'],['docs','https://docs.cbgpu.com/'],['preprints','https://arxiv.cbgpu.com/'],...[1,2,3,4].map(n=>['gpu'+n,'https://gpu.cbgpu.com/'+n+'/login'])].map(async([name,url])=>[name,await probe(url,name==='backend'?{Authorization:'Bearer '+env.MONITOR_KEY}:{})])));
 const backups=await env.OPS.get('backups','json')||[];checks.offsite_backup=!!backups[0]&&Date.now()-Date.parse(backups[0].created_at)<36*3600000;
 const previous=await env.OPS.get('monitor','json')||{failures:0,successes:0,incident:false};const healthy=Object.values(checks).every(Boolean);const state={...previous,checks,checked_at:new Date().toISOString(),failures:healthy?0:previous.failures+1,successes:healthy?previous.successes+1:0};
 if(send&&new Date().getUTCMinutes()%5===0)state.history=recordSample(previous.history,checks,state.checked_at);
 const transition=!state.incident&&state.failures>=3?'outage':state.incident&&state.successes>=2?'recovered':null;
 if(transition&&send){const subject=transition==='outage'?'CBGPU service needs attention':'CBGPU service recovered';const response=await fetch('https://api.resend.com/emails',{method:'POST',headers:{Authorization:'Bearer '+env.RESEND_KEY,'Content-Type':'application/json','Idempotency-Key':'monitor-'+transition+'-'+(previous.incident_id||new Date().toISOString().slice(0,16))},body:JSON.stringify({from:'CBGPU <info@cbgpu.com>',to:JSON.parse(env.RECIPIENTS),subject,html:`<div style="font-family:Arial,sans-serif;padding:28px;max-width:560px"><img alt="CBGPU" width="110" src="https://cbgpu.com/img/logo.png"><h1 style="font-size:24px">${subject}</h1><p>An external monitor checked the services.</p><ul>${Object.entries(checks).map(([name,ok])=>`<li>${name}: ${ok?'healthy':'unreachable'}</li>`).join('')}</ul><p>${state.checked_at}</p><a style="display:inline-block;background:#004fe8;color:white;padding:12px 18px;border-radius:8px;text-decoration:none" href="https://developers.cbgpu.com">Open developer panel</a></div>`})});if(response.ok){state.incident=transition==='outage';state.incident_id=state.incident?Date.now():null;state.last_alert_at=state.checked_at}else state.alert_error='Email provider rejected the alert';await response.body?.cancel()}
 if(!send||transition&&state.failures===3||state.failures>0&&state.failures<=3&&!previous.incident||previous.incident&&state.successes>0&&state.successes<=2||new Date().getUTCMinutes()%5===0)await env.OPS.put('monitor',JSON.stringify(state));return state;
}

// ---- Public status page -------------------------------------------------------------------------------------
// Only generic component names, up/down, check time and uptime percentages leave this Worker. No URLs, counters,
// backup entries or error text. Uptime comes from one sample every 5 minutes (written together with the state).
export const PUBLIC_COMPONENTS=[['website','웹사이트'],['admin','관리자'],['developers','개발자 포털'],['backend','백엔드'],['docs','문서'],['preprints','연구 프리프린트'],['gpu1','GPU 1'],['gpu2','GPU 2'],['gpu3','GPU 3'],['gpu4','GPU 4']];
const HISTORY_DAYS=90, STALE_MS=10*60000;
const kstDay=(ms)=>new Date(ms+9*3600000).toISOString().slice(0,10);
export function recordSample(history,checks,checkedAt){
 const next={...(history||{})};
 const day=kstDay(Date.parse(checkedAt)),today={...(next[day]||{})};
 for(const [id] of PUBLIC_COMPONENTS){const [up,total]=today[id]||[0,0];today[id]=[up+(checks[id]===true?1:0),total+1]}
 next[day]=today;
 for(const key of Object.keys(next).sort().slice(0,-HISTORY_DAYS))delete next[key];
 return next;
}
function percent(up,total){return total>0?Math.round(up/total*10000)/100:null}
export function uptime(history,id,days,now=Date.now()){
 let up=0,total=0;
 for(let i=0;i<days;i++){const day=(history||{})[kstDay(now-i*86400000)];if(!day)continue;for(const [component,counts] of Object.entries(day)){if(id&&component!==id)continue;up+=counts[0];total+=counts[1]}}
 return percent(up,total);
}
export function buildPublicStatus(state,now=Date.now()){
 const checkedAt=state?.checked_at?Date.parse(state.checked_at):NaN;
 const stale=!Number.isFinite(checkedAt)||now-checkedAt>STALE_MS;
 const history=state?.history||{};
 const components=PUBLIC_COMPONENTS.map(([id,name])=>({id,name,up:stale||!state?.checks?null:state.checks[id]===true,uptime_30d:uptime(history,id,30,now),uptime_90d:uptime(history,id,90,now)}));
 const down=components.some(component=>component.up===false);
 const overall=stale?'unknown':state.incident?'outage':down?'degraded':'operational';
 const days=Array.from({length:HISTORY_DAYS},(_,i)=>{const at=now-(HISTORY_DAYS-1-i)*86400000;return {date:kstDay(at),uptime:uptime(history,null,1,at)}});
 return {overall,checked_at:Number.isFinite(checkedAt)?new Date(checkedAt).toISOString():null,generated_at:new Date(now).toISOString(),components,days};
}
const esc=(value)=>String(value).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const LABEL={operational:['모든 서비스가 정상이에요','ok'],degraded:['일부 서비스에 문제가 있어요','warn'],outage:['서비스 장애를 확인하고 있어요','bad'],unknown:['상태 점검이 지연되고 있어요','unknown']};
export function renderStatusPage(status){
 const [headline,tone]=LABEL[status.overall]||LABEL.unknown;
 const pct=(value)=>value===null?'—':value.toFixed(2)+'%';
 const rows=status.components.map(c=>`<li><span class="dot ${c.up===null?'unknown':c.up?'ok':'bad'}"></span><span class="name">${esc(c.name)}</span><span class="state">${c.up===null?'확인 중':c.up?'정상':'문제 있음'}</span><span class="pct">${pct(c.uptime_90d)}</span></li>`).join('');
 const bars=status.days.map(d=>{const tone=d.uptime===null?'unknown':d.uptime>=99.9?'ok':d.uptime>=99?'warn':'bad';return `<i class="${tone}" title="${esc(d.date)} · ${d.uptime===null?'기록 없음':d.uptime.toFixed(2)+'%'}"></i>`}).join('');
 const checked=status.checked_at?new Date(status.checked_at).toLocaleString('ko-KR',{timeZone:'Asia/Seoul'}):'기록 없음';
 return `<!doctype html><html lang="ko"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta http-equiv="refresh" content="60"><title>CBGPU 서비스 상태</title><style>
:root{--bg:#fff;--card:#f6f7f9;--fg:#191f28;--muted:#6b7684;--line:#e5e8eb;--ok:#16a34a;--warn:#d97706;--bad:#dc2626;--unknown:#9ca3af}
@media(prefers-color-scheme:dark){:root{--bg:#17171c;--card:#202027;--fg:#f2f4f6;--muted:#a1a8b3;--line:#2d2d36}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.6 system-ui,-apple-system,"Noto Sans KR",sans-serif}
main{max-width:720px;margin:0 auto;padding:40px 16px 64px}h1{font-size:15px;font-weight:600;color:var(--muted);margin:0 0 12px}
.banner{border:1px solid var(--line);border-radius:18px;padding:22px;background:var(--card)}.banner b{display:block;font-size:22px;letter-spacing:-.02em}.banner small{color:var(--muted)}
.banner.ok b{color:var(--ok)}.banner.warn b{color:var(--warn)}.banner.bad b{color:var(--bad)}.banner.unknown b{color:var(--muted)}
section{margin-top:28px}h2{font-size:15px;margin:0 0 10px}ul{list-style:none;margin:0;padding:0;border:1px solid var(--line);border-radius:18px;overflow:hidden}
li{display:grid;grid-template-columns:14px 1fr auto 76px;gap:10px;align-items:center;padding:14px 18px;background:var(--card);border-top:1px solid var(--line)}li:first-child{border-top:0}
.dot{width:10px;height:10px;border-radius:50%}.dot.ok,i.ok{background:var(--ok)}.dot.bad,i.bad{background:var(--bad)}.dot.unknown,i.unknown{background:var(--unknown)}i.warn{background:var(--warn)}
.state{color:var(--muted);font-size:14px}.pct{text-align:right;font-variant-numeric:tabular-nums;font-size:14px}
.bars{display:flex;gap:2px;height:34px;align-items:stretch}.bars i{flex:1;border-radius:2px;min-width:2px}.legend{display:flex;justify-content:space-between;color:var(--muted);font-size:13px;margin-top:6px}
footer{margin-top:28px;color:var(--muted);font-size:13px}@media(max-width:480px){li{grid-template-columns:14px 1fr 64px}.state{display:none}}
</style></head><body><main><h1>CBGPU 서비스 상태</h1><div class="banner ${tone}"><b>${esc(headline)}</b><small>마지막 점검 ${esc(checked)} (KST)</small></div>
<section><h2>서비스 · 최근 90일 가동률</h2><ul>${rows}</ul></section>
<section><h2>하루별 가동률</h2><div class="bars">${bars}</div><div class="legend"><span>90일 전</span><span>오늘</span></div></section>
<footer>외부 점검기가 1분마다 서비스에 접속해 확인하고, 가동률은 5분마다 기록한 값으로 계산해요. 이 페이지는 1분마다 새로고침돼요.</footer></main></body></html>`;
}

export default{
 async scheduled(event,env,ctx){ctx.waitUntil(check(env))},
 async fetch(request,env){const url=new URL(request.url);if(url.pathname==='/health')return json({ok:true});
  if(request.method==='GET'&&(url.pathname==='/'||url.pathname==='/api/status')){const status=buildPublicStatus(await env.OPS.get('monitor','json'));const headers={'Cache-Control':'public, max-age=30','X-Content-Type-Options':'nosniff'};return url.pathname==='/'?new Response(renderStatusPage(status),{headers:{...headers,'Content-Type':'text/html; charset=utf-8','Content-Security-Policy':"default-src 'none'; style-src 'unsafe-inline'; img-src data:"}}):new Response(JSON.stringify(status),{headers:{...headers,'Content-Type':'application/json','Access-Control-Allow-Origin':'*'}})}if(!await keyOK(request.headers.get('Authorization')?.replace(/^Bearer /,''),env.OPS_KEY))return json({error:'Unauthorized'},401);
  if(url.pathname==='/status')return json({monitor:await env.OPS.get('monitor','json'),backups:await env.OPS.get('backups','json')||[]});
  if(url.pathname==='/monitor/check'&&request.method==='POST')return json(await check(env,false));
  const match=url.pathname.match(/^\/backups\/(\d{8}T\d{6}Z)(?:\/(manifest|\d{1,3}))?$/);if(!match)return json({error:'Not found'},404);const [,id,part]=match,key='backup:'+id+':'+(part||'manifest');
  if(request.method==='GET'){const result=await env.OPS.get(key,part==='manifest'||!part?'text':'arrayBuffer');return result===null?json({error:'Not found'},404):new Response(result,{headers:{'Content-Type':part==='manifest'||!part?'application/json':'application/octet-stream','Cache-Control':'no-store'}})}
  if(request.method!=='PUT')return json({error:'Method not allowed'},405);
  if(part==='manifest'){
   const body=await request.json();if(body.id!==id||body.algorithm!=='AES-256-GCM'||!Array.isArray(body.chunks)||!body.chunks.length||body.chunks.length>20||body.total_bytes>160*1024*1024)return json({error:'Invalid manifest'},400);
   await env.OPS.put(key,JSON.stringify(body),{expirationTtl:8*86400});const index=(await env.OPS.get('backups','json')||[]).filter(x=>x.id!==id);index.unshift({id,created_at:body.created_at,bytes:body.total_bytes,chunks:body.chunks.length});index.sort((a,b)=>b.id.localeCompare(a.id));const removed=index.splice(3);await env.OPS.put('backups',JSON.stringify(index));for(const old of removed){await env.OPS.delete('backup:'+old.id+':manifest');for(let i=0;i<old.chunks;i++)await env.OPS.delete('backup:'+old.id+':'+i)}return json({ok:true,id});
  }
  if(!part||Number(part)>19)return json({error:'Invalid chunk'},400);const bytes=await request.arrayBuffer();if(bytes.byteLength>8*1024*1024)return json({error:'Chunk too large'},413);await env.OPS.put(key,bytes,{expirationTtl:8*86400});return json({ok:true});
 }
};
