// Deployed to the runtime folder by the delivery agent (operations/delivery: sync_functions).
// Called by AFTER INSERT triggers on profiles (new signup) and reservations, and by monitoring scripts (ops_alert)
// (new GPU request) — see ../../notify_triggers_migration.sql. Notifies
// every admin (profiles.is_admin = true). Android-only, same FCM setup as
// the rest of the app.
import { createClient } from 'npm:@supabase/supabase-js@2.112.4';
import { corsHeaders, handleOptions } from '../_shared/cors.ts';
import { getFcmAccessToken, sendFcmPush } from '../_shared/fcm.ts';
import { requireServiceRole } from '../_shared/internalAuth.ts';

const supabaseUrl = Deno.env.get('SUPABASE_URL')!;
const serviceRoleKey = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY')!;

async function isTestSignup(fullName: unknown, studentId: unknown): Promise<boolean> {
  if (typeof fullName !== 'string' || !fullName) return false;
  try {
    const client = createClient(supabaseUrl, serviceRoleKey);
    let query = client.from('profiles').select('id').eq('full_name', fullName).limit(5);
    query = typeof studentId === 'string' && studentId ? query.eq('student_id', studentId) : query.is('student_id', null);
    const { data: rows } = await query;
    if (!rows?.length) return false;
    for (const row of rows as { id: string }[]) {
      const { data } = await client.auth.admin.getUserById(row.id);
      if (!/@example\.invalid$/i.test(data?.user?.email ?? '')) return false;
    }
    return true;
  } catch {
    return false; // when unsure, notify
  }
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;
  if (!requireServiceRole(req)) return Response.json({ message: 'forbidden' }, { status: 401, headers: corsHeaders });

  // deno-lint-ignore no-explicit-any
  const payload = await req.json().catch(() => ({}) as any);
  const { type } = payload;

  let title: string;
  let body: string;
  let url: string;
  if (type === 'signup') {
    // Automated tests create throw-away accounts on @example.invalid (reserved, can never receive mail). They are
    // removed within seconds, so a push for them is just noise: skip when every matching account is such a one.
    if (await isTestSignup(payload.full_name, payload.student_id)) {
      return Response.json({ ok: true, notified: 0, reason: 'test account' }, { headers: corsHeaders });
    }
    const label = payload.full_name ? `${payload.full_name}(${payload.student_id ?? '학번 미입력'})` : '새 사용자';
    title = '새 회원가입';
    body = `${label}님이 가입했습니다. 승인이 필요합니다.`;
    url = 'https://admin.cbgpu.com/admin/m?tab=signups&from=app';
  } else if (type === 'gpu_request') {
    title = 'GPU 예약 신청';
    body = `GPU #${payload.gpu_id} 예약 신청이 들어왔습니다.`;
    url = 'https://admin.cbgpu.com/admin/m?tab=gpu&from=app';
  } else if (type === 'ops_alert') {
    // Operator alerts (school connection expiring/expired, health probe failing, ...). Sent by our own
    // monitoring scripts with the internal secret; text is length-limited and the link must be ours.
    title = String(payload.title ?? '').slice(0, 60);
    body = String(payload.body ?? '').slice(0, 160);
    if (!title || !body) return Response.json({ message: 'title and body are required' }, { status: 400, headers: corsHeaders });
    url = typeof payload.url === 'string' && /^https:\/\/(admin|app)\.cbgpu\.com\//.test(payload.url)
      ? payload.url
      : 'https://admin.cbgpu.com/admin/m?from=app';
  } else {
    return Response.json({ message: 'invalid type' }, { status: 400, headers: corsHeaders });
  }

  const admin = createClient(supabaseUrl, serviceRoleKey);
  const { data: admins } = await admin.from('profiles').select('id').eq('is_admin', true);
  const adminIds = (admins ?? []).map((a: { id: string }) => a.id);
  if (adminIds.length === 0) return Response.json({ ok: true, notified: 0, reason: 'no admins' }, { headers: corsHeaders });

  const { data: tokenRows } = await admin.from('device_tokens').select('token').in('user_id', adminIds).eq('platform', 'android');
  const deviceTokens = (tokenRows ?? []).map((t: { token: string }) => t.token);
  if (deviceTokens.length === 0) return Response.json({ ok: true, notified: 0, reason: 'no device tokens' }, { headers: corsHeaders });

  const firebaseServiceAccountRaw = Deno.env.get('FIREBASE_SERVICE_ACCOUNT_JSON');
  if (!firebaseServiceAccountRaw) return Response.json({ message: 'FIREBASE_SERVICE_ACCOUNT_JSON secret이 설정되어 있지 않습니다.' }, { status: 500, headers: corsHeaders });
  const serviceAccount = JSON.parse(firebaseServiceAccountRaw);
  const accessToken = await getFcmAccessToken(serviceAccount);

  let notified = 0;
  let failed = 0;
  for (const deviceToken of deviceTokens) {
    try {
      await sendFcmPush(serviceAccount.project_id, accessToken, deviceToken, title, body, undefined, { url });
      notified++;
    } catch {
      failed++;
    }
  }

  return Response.json({ ok: true, notified, failed }, { headers: corsHeaders });
});
