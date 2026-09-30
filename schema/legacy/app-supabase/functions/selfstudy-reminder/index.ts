// One cron invocation every 15 minutes; all user-facing times are KST.
import { createClient } from 'npm:@supabase/supabase-js@2.112.4';
import crypto from 'node:crypto';
import { Buffer } from 'node:buffer';
import { getFcmAccessToken } from '../_shared/fcm.ts';

const API = 'https://api.cbsh.edu-set.com';
type Device = { token: string; platform: string };
type Profile = {
  id: string; student_id: string | null; selfstudy_reminder_time: string;
  selfstudy_reminder_repeat_minutes: number; selfstudy_reminder_repeat_count: number;
  device_tokens: Device[];
};
type Attempt = { user_id: string; attempt: number; claimed_at: string };
type Policy = { timetable: { presets: Record<string, unknown[]>; assign: Record<string, string>; exception: Record<string, string> } };
type Student = { id: number; grade: number; classNo: number; number: number };
type Occupation = { studentId: number; period: number; state?: string };
type SearchEntry = { period: number; status: string; student: Student };

function kstNow() {
  const shifted = new Date(Date.now() + 9 * 60 * 60 * 1000);
  return { date: shifted.toISOString().slice(0, 10), minute: shifted.getUTCHours() * 60 + shifted.getUTCMinutes(), day: shifted.getUTCDay() };
}
function minutes(time: string) { const [h, m] = time.split(':').map(Number); return h * 60 + m; }
function studentNumber(student: Student) { return String(student.grade * 1000 + student.classNo * 100 + student.number).padStart(4, '0'); }
function decryptToken(payload: string, secret: string) {
  const raw = Buffer.from(payload, 'base64');
  const decipher = crypto.createDecipheriv('aes-256-gcm', crypto.createHash('sha256').update(secret).digest(), raw.subarray(0, 12));
  decipher.setAuthTag(raw.subarray(12, 28));
  return Buffer.concat([decipher.update(raw.subarray(28)), decipher.final()]).toString('utf8');
}
async function get<T>(path: string, token: string): Promise<T> {
  const response = await fetch(`${API}${path}`, { headers: { Authorization: `Bearer ${token}`, 'x-jwt': token }, signal: AbortSignal.timeout(12000) });
  if (!response.ok) throw new Error(`EduSet ${path}: HTTP ${response.status}`);
  return response.json();
}
async function appliedPeriods(token: string, date: string): Promise<Map<string, Set<number>>> {
  // Search includes pending applications and non-seat rooms when the shared
  // token can access it. The current token receives HTTP 500, so fall back to
  // the same per-room occupations used by the study page.
  try {
    const response = await fetch(`${API}/selfstudy/search`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}`, 'x-jwt': token, 'Content-Type': 'application/json' },
      body: JSON.stringify({ date }),
      signal: AbortSignal.timeout(12000),
    });
    if (response.ok) {
      const entries = await response.json() as SearchEntry[];
      const complete = new Map<string, Set<number>>();
      for (const entry of entries) {
        if (entry.status === 'REJECTED') continue;
        const number = studentNumber(entry.student);
        if (!complete.has(number)) complete.set(number, new Set());
        complete.get(number)!.add(entry.period);
      }
      return complete;
    }
  } catch {
    // Per-room fallback below.
  }
  const [rooms, students] = await Promise.all([get<{ id: number }[]>('/rooms', token), get<Student[]>('/students', token)]);
  const numbers = new Map(students.map(student => [student.id, studentNumber(student)]));
  const applied = new Map<string, Set<number>>();
  // A failed room makes absence uncertain. Fail the run instead of sending a false reminder.
  for (let offset = 0; offset < rooms.length; offset += 8) {
    const occupations = await Promise.all(rooms.slice(offset, offset + 8).map(room =>
      get<Occupation[]>(`/rooms/${room.id}/seats/occupations?date=${date}`, token)));
    for (const room of occupations) for (const occupation of room) {
      if (occupation.state === 'REJECTED') continue;
      const number = numbers.get(occupation.studentId);
      if (!number) throw new Error(`Unknown EduSet student ${occupation.studentId}`);
      if (!applied.has(number)) applied.set(number, new Set());
      applied.get(number)!.add(occupation.period);
    }
  }
  return applied;
}
async function sendPush(projectId: string, accessToken: string, device: Device, body: string) {
  const response = await fetch(`https://fcm.googleapis.com/v1/projects/${projectId}/messages:send`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ message: {
      token: device.token, notification: { title: '자습신청 알림', body },
      data: { route: '/study?reminder=1' },
      ...(device.platform === 'web' ? { webpush: { fcm_options: { link: 'https://cbgpu.vercel.app/study?reminder=1' } } } : {}),
    } }),
    signal: AbortSignal.timeout(12000),
  });
  if (!response.ok) throw new Error(`FCM HTTP ${response.status}: ${await response.text()}`);
}

Deno.serve(async request => {
  const serviceRoleKey = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY');
  const url = Deno.env.get('SUPABASE_URL');
  const secret = Deno.env.get('EDUSET_TOKEN_SECRET');
  const firebaseRaw = Deno.env.get('FIREBASE_SERVICE_ACCOUNT_JSON');
  if (!serviceRoleKey || !url || !secret || !firebaseRaw) return Response.json({ error: 'Missing server configuration' }, { status: 500 });
  if (request.headers.get('Authorization') !== `Bearer ${serviceRoleKey}`) return Response.json({ error: 'Unauthorized' }, { status: 401 });
  const now = kstNow();
  if (now.minute < 8 * 60 || now.minute > 15 * 60 + 15) return Response.json({ ok: true, skipped: 'outside hours' });

  const admin = createClient(url, serviceRoleKey);
  const { data: profiles, error: profilesError } = await admin.from('profiles')
    .select('id, student_id, selfstudy_reminder_time, selfstudy_reminder_repeat_minutes, selfstudy_reminder_repeat_count, device_tokens(token, platform)')
    .eq('notify_selfstudy_reminder', true).neq('is_approved', false).not('student_id', 'is', null);
  if (profilesError) return Response.json({ error: profilesError.message }, { status: 500 });
  const eligible = ((profiles ?? []) as Profile[]).filter(profile =>
    /^\d{4}$/.test(profile.student_id ?? '') && minutes(profile.selfstudy_reminder_time) <= now.minute &&
    (profile.device_tokens ?? []).some(device => ['android', 'ios'].includes(device.platform)));
  if (!eligible.length) return Response.json({ ok: true, due: 0 });

  const ids = eligible.map(profile => profile.id);
  const [{ data: attempts, error: attemptsError }, { data: acks, error: ackError }] = await Promise.all([
    admin.from('selfstudy_reminder_attempts').select('user_id, attempt, claimed_at').eq('study_date', now.date).in('user_id', ids),
    admin.from('selfstudy_reminder_acknowledgements').select('user_id').eq('study_date', now.date).in('user_id', ids),
  ]);
  if (attemptsError || ackError) return Response.json({ error: attemptsError?.message ?? ackError?.message }, { status: 500 });
  const acknowledged = new Set((acks ?? []).map(row => row.user_id));
  const history = new Map<string, Attempt[]>();
  for (const attempt of (attempts ?? []) as Attempt[]) {
    if (!history.has(attempt.user_id)) history.set(attempt.user_id, []);
    history.get(attempt.user_id)!.push(attempt);
  }
  const due = eligible.filter(profile => {
    if (acknowledged.has(profile.id)) return false;
    const previous = history.get(profile.id) ?? [];
    if (!previous.length) return true;
    if (previous.length > profile.selfstudy_reminder_repeat_count) return false;
    return Date.now() - Math.max(...previous.map(row => Date.parse(row.claimed_at))) >= profile.selfstudy_reminder_repeat_minutes * 60000;
  });
  if (!due.length) return Response.json({ ok: true, due: 0 });

  try {
    const { data: settings, error: settingsError } = await admin.from('eduset_settings').select('access_token_enc').eq('id', 1).single();
    if (settingsError || !settings?.access_token_enc) throw new Error('EduSet token unavailable');
    const token = decryptToken(settings.access_token_enc, secret);
    const policy = (await get<{ data: Policy }>('/policies/selfstudy', token)).data;
    const preset = policy.timetable.exception[now.date] ?? policy.timetable.assign[String(now.day)];
    const count = preset ? (policy.timetable.presets[preset]?.length ?? 0) : 0;
    if (!count) return Response.json({ ok: true, skipped: 'no self-study periods today' });
    const applied = await appliedPeriods(token, now.date);
    const firebase = JSON.parse(firebaseRaw) as { project_id: string; client_email: string; private_key: string };
    const accessToken = await getFcmAccessToken(firebase);
    let notified = 0, failed = 0;
    for (const profile of due) {
      const mine = applied.get(profile.student_id!) ?? new Set<number>();
      const missing = Array.from({ length: count }, (_, index) => index + 1).filter(period => !mine.has(period));
      if (!missing.length) continue;
      // Recheck acknowledgement after the potentially long upstream lookup.
      const { data: ack, error: recheckError } = await admin.from('selfstudy_reminder_acknowledgements')
        .select('user_id').eq('user_id', profile.id).eq('study_date', now.date).maybeSingle();
      if (recheckError) throw recheckError;
      if (ack) continue;
      const attempt = (history.get(profile.id) ?? []).length;
      const { error: claimError } = await admin.from('selfstudy_reminder_attempts').insert({ user_id: profile.id, study_date: now.date, attempt });
      if (claimError) { failed++; continue; }
      const body = `${missing.map(period => `${period}교시`).join(', ')} 자습신청이 확인되지 않았어요.`;
      const outcomes = await Promise.allSettled((profile.device_tokens ?? []).filter(device => ['android', 'ios'].includes(device.platform))
        .map(device => sendPush(firebase.project_id, accessToken, device, body)));
      if (outcomes.some(result => result.status === 'fulfilled')) notified++;
      else {
        await admin.from('selfstudy_reminder_attempts').delete().eq('user_id', profile.id).eq('study_date', now.date).eq('attempt', attempt);
        failed++;
      }
    }
    return Response.json({ ok: true, date: now.date, due: due.length, notified, failed });
  } catch (error) {
    return Response.json({ error: (error as Error).message }, { status: 502 });
  }
});
