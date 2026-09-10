// Called by an AFTER INSERT trigger on print_jobs/laser_jobs (see
// ../../notify_triggers_migration.sql). Notifies whoever should act on a
// freshly submitted manufacturing job:
//   - if the requester picked a specific manager, just that person
//   - otherwise, every manager of the target printer (print) or every
//     laser-cut manager (laser) — same "first to accept gets it" pool the
//     Admin/Manufacturing screens already assume.
// Android-only, matching the FCM setup used everywhere else in this app.
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
  const { type, job_id } = await req.json().catch(() => ({}) as any);
  if (type !== 'print' && type !== 'laser' || !job_id) {
    return Response.json({ message: 'invalid payload' }, { status: 400, headers: corsHeaders });
  }

  const admin = createClient(supabaseUrl, serviceRoleKey);
  const table = type === 'print' ? 'print_jobs' : 'laser_jobs';
  const { data: job } = await admin.from(table).select('*, requester:user_id(full_name, student_id)').eq('id', job_id).maybeSingle();
  if (!job) return Response.json({ message: 'job not found' }, { status: 404, headers: corsHeaders });

  let targetUserIds: string[] = [];
  if (job.manager_id) {
    targetUserIds = [job.manager_id];
  } else if (type === 'print' && job.preferred_printer_id != null) {
    const { data: pm } = await admin.from('printer_managers').select('user_id').eq('printer_id', job.preferred_printer_id);
    targetUserIds = (pm ?? []).map((r: { user_id: string }) => r.user_id);
  } else if (type === 'laser') {
    const { data: managers } = await admin.from('profiles').select('id').eq('is_laser_cut_manager', true);
    targetUserIds = (managers ?? []).map((r: { id: string }) => r.id);
  }

  if (targetUserIds.length === 0) return Response.json({ ok: true, notified: 0, reason: 'no managers to notify' }, { headers: corsHeaders });

  const { data: optedInProfiles } = await admin.from('profiles').select('id').in('id', targetUserIds).eq('notify_job_new', true);
  targetUserIds = (optedInProfiles ?? []).map((p: { id: string }) => p.id);
  if (targetUserIds.length === 0) return Response.json({ ok: true, notified: 0, reason: 'managers opted out' }, { headers: corsHeaders });

  const { data: tokenRows } = await admin.from('device_tokens').select('token').in('user_id', targetUserIds).eq('platform', 'android');
  const deviceTokens = (tokenRows ?? []).map((t: { token: string }) => t.token);
  if (deviceTokens.length === 0) return Response.json({ ok: true, notified: 0, reason: 'no device tokens' }, { headers: corsHeaders });

  const firebaseServiceAccountRaw = Deno.env.get('FIREBASE_SERVICE_ACCOUNT_JSON');
  if (!firebaseServiceAccountRaw) return Response.json({ message: 'FIREBASE_SERVICE_ACCOUNT_JSON secret이 설정되어 있지 않습니다.' }, { status: 500, headers: corsHeaders });
  const serviceAccount = JSON.parse(firebaseServiceAccountRaw);
  const accessToken = await getFcmAccessToken(serviceAccount);

  const requesterLabel = job.requester?.full_name ? `${job.requester.full_name}(${job.requester.student_id ?? '-'})` : '누군가';
  const title = type === 'print' ? '새 3D프린팅 신청' : '새 레이저 커팅 신청';
  const body = type === 'print'
    ? `${requesterLabel}님이 "${job.file_name ?? '파일'}" 인쇄를 신청했습니다.`
    : `${requesterLabel}님이 ${job.material === 'mdf' ? 'MDF' : '아크릴'} ${job.thickness_mm ?? ''}mm 커팅을 신청했습니다.`;

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
