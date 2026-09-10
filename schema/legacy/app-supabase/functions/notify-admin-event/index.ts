// Called by AFTER INSERT triggers on profiles (new signup) and reservations
// (new GPU request) — see ../../notify_triggers_migration.sql. Notifies
// every admin (profiles.is_admin = true). Android + iOS, same FCM setup as
// the rest of the app.
import { createClient } from 'npm:@supabase/supabase-js@2.112.4';
import { corsHeaders, handleOptions } from '../_shared/cors.ts';
import { getFcmAccessToken, sendFcmPush } from '../_shared/fcm.ts';
import { requireServiceRole } from '../_shared/internalAuth.ts';

const supabaseUrl = Deno.env.get('SUPABASE_URL')!;
const serviceRoleKey = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY')!;

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;
  if (!requireServiceRole(req)) return Response.json({ message: 'forbidden' }, { status: 401, headers: corsHeaders });

  // deno-lint-ignore no-explicit-any
  const payload = await req.json().catch(() => ({}) as any);
  const { type } = payload;

  let title: string;
  let body: string;
  if (type === 'signup') {
    const label = payload.full_name ? `${payload.full_name}(${payload.student_id ?? '학번 미입력'})` : '새 사용자';
    title = '새 회원가입';
    body = `${label}님이 가입했습니다. 승인이 필요합니다.`;
  } else if (type === 'gpu_request') {
    title = 'GPU 예약 신청';
    body = `GPU #${payload.gpu_id} 예약 신청이 들어왔습니다.`;
  } else {
    return Response.json({ message: 'invalid type' }, { status: 400, headers: corsHeaders });
  }

  const prefColumn = type === 'signup' ? 'notify_admin_signup' : 'notify_admin_gpu_request';
  const admin = createClient(supabaseUrl, serviceRoleKey);
  const { data: admins } = await admin.from('profiles').select('id').eq('is_admin', true).eq(prefColumn, true);
  const adminIds = (admins ?? []).map((a: { id: string }) => a.id);
  if (adminIds.length === 0) return Response.json({ ok: true, notified: 0, reason: 'no admins opted in' }, { headers: corsHeaders });

  const { data: tokenRows } = await admin.from('device_tokens').select('token').in('user_id', adminIds).in('platform', ['android', 'ios']);
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
      await sendFcmPush(serviceAccount.project_id, accessToken, deviceToken, title, body);
      notified++;
    } catch {
      failed++;
    }
  }

  return Response.json({ ok: true, notified, failed }, { headers: corsHeaders });
});
