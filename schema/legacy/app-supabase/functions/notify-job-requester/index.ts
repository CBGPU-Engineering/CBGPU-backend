// Called by an AFTER UPDATE trigger on print_jobs/laser_jobs when status
// transitions to 'done' (see ../../notify_triggers_migration.sql). The
// mirror image of notify-job-manager: that one tells the manager a job came
// in, this one tells the original requester it's ready to collect.
// Android + iOS, matching the FCM setup used everywhere else in this app.
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
  if ((type !== 'print' && type !== 'laser') || !job_id) {
    return Response.json({ message: 'invalid payload' }, { status: 400, headers: corsHeaders });
  }

  const admin = createClient(supabaseUrl, serviceRoleKey);
  const table = type === 'print' ? 'print_jobs' : 'laser_jobs';
  const { data: job } = await admin.from(table).select('*, requester:user_id(notify_job_done)').eq('id', job_id).maybeSingle();
  if (!job) return Response.json({ message: 'job not found' }, { status: 404, headers: corsHeaders });

  if (job.requester?.notify_job_done === false) {
    return Response.json({ ok: true, notified: 0, reason: 'requester opted out' }, { headers: corsHeaders });
  }

  const { data: tokenRows } = await admin.from('device_tokens').select('token').eq('user_id', job.user_id).in('platform', ['android', 'ios']);
  const deviceTokens = (tokenRows ?? []).map((t: { token: string }) => t.token);
  if (deviceTokens.length === 0) return Response.json({ ok: true, notified: 0, reason: 'no device tokens' }, { headers: corsHeaders });

  const firebaseServiceAccountRaw = Deno.env.get('FIREBASE_SERVICE_ACCOUNT_JSON');
  if (!firebaseServiceAccountRaw) return Response.json({ message: 'FIREBASE_SERVICE_ACCOUNT_JSON secret이 설정되어 있지 않습니다.' }, { status: 500, headers: corsHeaders });
  const serviceAccount = JSON.parse(firebaseServiceAccountRaw);
  const accessToken = await getFcmAccessToken(serviceAccount);

  const title = type === 'print' ? '3D프린팅 완료' : '레이저 커팅 완료';
  const body = type === 'print'
    ? `"${job.file_name ?? '파일'}" 인쇄가 완료됐어요. 수령해주세요!`
    : `${job.material === 'mdf' ? 'MDF' : '아크릴'} ${job.thickness_mm ?? ''}mm 커팅이 완료됐어요. 수령해주세요!`;

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
