// 오늘의 자습 감독을 외부 Google Apps Script에서 가져와 study_supervisors 테이블에 저장한다.
// 호출하는 건 Supabase pg_cron(1시간마다)뿐이고, 사용자(웹/앱)는 저장된 값을 읽기만 한다 —
// 접속자 수가 외부 스크립트 호출 횟수에 영향을 주지 않는다. (supabase_study_supervisors_migration.sql 참고)
//
// 이 스크립트는 JSON이 아니라 HTML 화면을 돌려주고, 담당자 이름은 화면이 열린 뒤 google.script.run이 따로
// 가져와 그린다. 그래서 그 화면이 보내는 요청(/callback)을 그대로 보내 결과 JSON만 받는다. 구글의 비공개 내부
// 방식이라 바뀌면 실패한다 — 그때는 HTTP 502로 응답하고, 저장된 이전 값은 그대로 둔다.
import { createClient } from 'npm:@supabase/supabase-js@2.112.4';

const SCRIPT_ID = 'AKfycbzQJ6tHq5yhgnfN_TjL96zSUD8ug28famlojcv9J-6kDTMqfVkezZgcikD7JOyywePu';
const SCRIPT_URL = `https://script.google.com/macros/s/${SCRIPT_ID}/exec`;

// 스케줄이 잘못 설정돼 너무 자주 불려도 남의 스크립트를 두드리지 않도록 이 안에서는 다시 가져오지 않는다.
const MIN_INTERVAL_MS = 10 * 60_000;

type Duty = { type: string; teacher: string };
type Snapshot = { dateLabel: string; duties: Duty[] };

const clean = (text: string, max: number) => text.replace(/\s+/g, ' ').trim().slice(0, max);

/** 'YYYY-MM-DD' (한국 시간 기준) */
function todayKst(): string {
  return new Intl.DateTimeFormat('sv-SE', { timeZone: 'Asia/Seoul' }).format(new Date());
}

async function fetchSupervisors(): Promise<Snapshot> {
  const response = await fetch(`${SCRIPT_URL.replace(/exec$/, 'callback')}?nocache_id=6`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/x-www-form-urlencoded;charset=UTF-8',
      'X-Same-Domain': '1',
      Referer: SCRIPT_URL,
    },
    body: new URLSearchParams({ request: JSON.stringify(['getTodayDuty', '[]', null, [0], null, null, 1, 0]) }),
    signal: AbortSignal.timeout(15_000),
  });
  if (!response.ok) throw new Error(`자습 감독 요청이 실패했습니다 (HTTP ${response.status}).`);

  // 응답 앞의 )]}' 는 JSON 하이재킹 방지용 접두어다.
  const envelope = JSON.parse((await response.text()).replace(/^\)\]\}'\s*/, ''));
  const exec = Array.isArray(envelope) ? envelope.find((entry) => entry?.[0] === 'op.exec') : null;
  if (!exec || exec[1]?.[0] !== 0 || typeof exec[1]?.[1] !== 'string') throw new Error('자습 감독 응답을 해석하지 못했습니다.');

  const payload = JSON.parse(exec[1][1]);
  if (typeof payload?.date !== 'string' || !Array.isArray(payload?.duties)) throw new Error('자습 감독 응답 형식이 예상과 다릅니다.');
  return {
    dateLabel: clean(payload.date, 60),
    duties: payload.duties.slice(0, 20).map((duty: { type?: unknown; teacher?: unknown }) => ({
      type: clean(String(duty?.type ?? ''), 40),
      teacher: clean(String(duty?.teacher ?? '').replace(/\s*선생님$/, ''), 40),
    })).filter((duty: Duty) => duty.type || duty.teacher),
  };
}

Deno.serve(async (request) => {
  const serviceRoleKey = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY');
  const url = Deno.env.get('SUPABASE_URL');
  if (!serviceRoleKey || !url) return Response.json({ error: 'Missing server configuration' }, { status: 500 });
  // selfstudy-reminder와 같은 방식: pg_cron이 Vault의 서비스 키를 Authorization으로 보낸다.
  if (request.headers.get('Authorization') !== `Bearer ${serviceRoleKey}`) return Response.json({ error: 'Unauthorized' }, { status: 401 });

  const db = createClient(url, serviceRoleKey, { auth: { persistSession: false } });

  const { data: row, error: readError } = await db.from('study_supervisors').select('fetched_at').eq('id', 1).maybeSingle();
  if (readError || !row) {
    return Response.json({ error: `study_supervisors를 읽지 못했습니다 (마이그레이션을 실행했나요?): ${readError?.message ?? '행 없음'}` }, { status: 500 });
  }
  if (Date.now() - new Date(row.fetched_at).getTime() < MIN_INTERVAL_MS) return Response.json({ ok: true, skipped: '최근에 이미 가져왔습니다.' });

  // fetched_at을 비교-교체(compare-and-swap)해서, 호출이 겹쳐도 하나만 실제로 외부를 호출한다.
  const { data: claimed, error: claimError } = await db.from('study_supervisors')
    .update({ fetched_at: new Date().toISOString() }).eq('id', 1).eq('fetched_at', row.fetched_at).select('id');
  if (claimError) return Response.json({ error: `갱신 권한을 얻지 못했습니다: ${claimError.message}` }, { status: 500 });
  if (!claimed?.length) return Response.json({ ok: true, skipped: '다른 요청이 이미 가져오는 중입니다.' });

  const dutyDate = todayKst();
  let snapshot: Snapshot;
  try {
    snapshot = await fetchSupervisors();
  } catch (error) {
    // 실패하면 이전 값을 그대로 둔다. fetched_at은 이미 갱신돼 있어서 MIN_INTERVAL_MS 동안은 다시 시도하지 않는다.
    const message = error instanceof Error ? error.message : String(error);
    console.error('[refresh-supervisors]', message);
    return Response.json({ error: message }, { status: 502 });
  }

  const { error: writeError } = await db.from('study_supervisors').update({
    duty_date: dutyDate, date_label: snapshot.dateLabel, duties: snapshot.duties, updated_at: new Date().toISOString(),
  }).eq('id', 1);
  if (writeError) return Response.json({ error: `저장하지 못했습니다: ${writeError.message}` }, { status: 500 });

  return Response.json({ ok: true, dutyDate, dateLabel: snapshot.dateLabel, dutyCount: snapshot.duties.length });
});
