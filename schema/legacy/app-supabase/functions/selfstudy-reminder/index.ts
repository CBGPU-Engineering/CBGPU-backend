// Supabase Edge Function (Deno) — scheduled by pg_cron (see ../notification_migration.sql)
// at 04:00 UTC (13:00 KST) daily. Checks whether each approved user has
// applied for 자습신청 today via the shared admin-registered eduset token
// (same one used by the web app's /study page — no per-user eduset login),
// and sends an Android FCM push to anyone who hasn't.
//
// Deliberately backend-agnostic of both client apps (Next.js web, Flutter):
// lives with the database, not with either frontend, since only the schema
// (profiles/eduset_settings/device_tokens) is a real shared dependency.
//
// Required secrets (supabase secrets set ...):
//   EDUSET_TOKEN_SECRET        — same value as the Next.js app's .env.local
//   FIREBASE_SERVICE_ACCOUNT_JSON — the Firebase service account key, as JSON text
// SUPABASE_URL / SUPABASE_SERVICE_ROLE_KEY are provided automatically by Supabase.

import { createClient } from 'npm:@supabase/supabase-js@2.112.4';
import crypto from 'node:crypto';
// Unlike real Node, Deno doesn't expose Buffer as a global — needs an explicit import.
import { Buffer } from 'node:buffer';

const EDUSET_BASE_URL = 'https://api.cbsh.edu-set.com';

function localDateStrKST(): string {
  const now = new Date(Date.now() + 9 * 60 * 60 * 1000);
  const y = now.getUTCFullYear();
  const m = String(now.getUTCMonth() + 1).padStart(2, '0');
  const d = String(now.getUTCDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

// Same AES-256-GCM scheme as src/lib/edusetTokenCrypto.ts in the Next.js app —
// must stay byte-for-byte identical, both apps decrypt the same stored value.
function decryptToken(payload: string, secret: string): string {
  const key = crypto.createHash('sha256').update(secret).digest();
  const raw = Buffer.from(payload, 'base64');
  const iv = raw.subarray(0, 12);
  const tag = raw.subarray(12, 28);
  const encrypted = raw.subarray(28);
  const decipher = crypto.createDecipheriv('aes-256-gcm', key, iv);
  decipher.setAuthTag(tag);
  const decrypted = Buffer.concat([decipher.update(encrypted), decipher.final()]);
  return decrypted.toString('utf8');
}

interface EdusetRoom {
  id: number;
  hasSeats: boolean;
}
interface EdusetSeatOccupation {
  seatId: number;
  period: number;
  studentId: number;
  state?: string;
}
interface EdusetStudent {
  id: number;
  grade: number;
  classNo: number;
  number: number;
}

/** Mirrors parseStudentId()'s encoding elsewhere in the codebase: grade*1000 + classNo*100 + number */
function toStudentId(s: { grade: number; classNo: number; number: number }): string {
  return String(s.grade * 1000 + s.classNo * 100 + s.number).padStart(4, '0');
}

async function edusetGet<T>(path: string, token: string): Promise<T> {
  const res = await fetch(`${EDUSET_BASE_URL}${path}`, {
    headers: { Authorization: `Bearer ${token}`, 'x-jwt': token },
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

/**
 * /selfstudy/search (a single call covering every room) 500s outright for
 * this admin-registered token's permission level — confirmed by testing
 * directly against the live API, not assumed. The web app's /study page
 * already hit this exact wall and works around it by falling back to each
 * room's occupations endpoint instead, which this account *can* read; ported
 * the same approach here rather than depending on the broken endpoint.
 */
async function getAppliedStudentIds(edusetToken: string, dateStr: string): Promise<Set<string>> {
  const rooms = await edusetGet<EdusetRoom[]>('/rooms', edusetToken);
  const applied = new Set<string>();
  const studentCache = new Map<number, EdusetStudent>();

  for (const room of rooms) {
    let occupations: EdusetSeatOccupation[];
    try {
      occupations = await edusetGet<EdusetSeatOccupation[]>(`/rooms/${room.id}/seats/occupations?date=${dateStr}`, edusetToken);
    } catch {
      continue; // this room's occupations aren't readable with this account either — skip, don't fail the whole run
    }

    const uniqueStudentIds = [...new Set(occupations.map((o) => o.studentId))];
    await Promise.all(
      uniqueStudentIds
        .filter((id) => !studentCache.has(id))
        .map(async (id) => {
          try {
            studentCache.set(id, await edusetGet<EdusetStudent>(`/students/${id}`, edusetToken));
          } catch {
            // couldn't resolve this student's grade/class/number — leave them unresolved
          }
        })
    );

    for (const o of occupations) {
      const student = studentCache.get(o.studentId);
      if (student) applied.add(toStudentId(student));
    }
  }

  return applied;
}

// --- FCM via raw REST + a service-account JWT, instead of firebase-admin
// (which depends on gRPC and isn't a good fit for the Deno edge runtime). ---

let cachedAccessToken: { token: string; expiresAt: number } | null = null;

async function getFcmAccessToken(serviceAccount: { client_email: string; private_key: string }): Promise<string> {
  if (cachedAccessToken && Date.now() < cachedAccessToken.expiresAt - 60_000) {
    return cachedAccessToken.token;
  }

  const now = Math.floor(Date.now() / 1000);
  const header = { alg: 'RS256', typ: 'JWT' };
  const claims = {
    iss: serviceAccount.client_email,
    scope: 'https://www.googleapis.com/auth/firebase.messaging',
    aud: 'https://oauth2.googleapis.com/token',
    iat: now,
    exp: now + 3600,
  };
  const base64url = (obj: unknown) => Buffer.from(JSON.stringify(obj)).toString('base64url');
  const unsigned = `${base64url(header)}.${base64url(claims)}`;
  const signature = crypto.sign('RSA-SHA256', Buffer.from(unsigned), serviceAccount.private_key).toString('base64url');
  const jwt = `${unsigned}.${signature}`;

  const res = await fetch('https://oauth2.googleapis.com/token', {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ grant_type: 'urn:ietf:params:oauth:grant-type:jwt-bearer', assertion: jwt }),
  });
  if (!res.ok) throw new Error(`FCM OAuth token exchange failed: HTTP ${res.status}`);
  const data = await res.json();
  cachedAccessToken = { token: data.access_token, expiresAt: Date.now() + data.expires_in * 1000 };
  return data.access_token;
}

async function sendFcmPush(projectId: string, accessToken: string, deviceToken: string, title: string, body: string) {
  const res = await fetch(`https://fcm.googleapis.com/v1/projects/${projectId}/messages:send`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ message: { token: deviceToken, notification: { title, body } } }),
  });
  if (!res.ok) throw new Error(`FCM send failed: HTTP ${res.status} ${await res.text()}`);
}

Deno.serve(async () => {
  const supabaseUrl = Deno.env.get('SUPABASE_URL')!;
  const serviceRoleKey = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY')!;
  const edusetTokenSecret = Deno.env.get('EDUSET_TOKEN_SECRET');
  const firebaseServiceAccountRaw = Deno.env.get('FIREBASE_SERVICE_ACCOUNT_JSON');

  if (!edusetTokenSecret) {
    return Response.json({ message: 'EDUSET_TOKEN_SECRET secret이 설정되어 있지 않습니다.' }, { status: 500 });
  }
  if (!firebaseServiceAccountRaw) {
    return Response.json({ message: 'FIREBASE_SERVICE_ACCOUNT_JSON secret이 설정되어 있지 않습니다.' }, { status: 500 });
  }

  const admin = createClient(supabaseUrl, serviceRoleKey);

  const { data: settings } = await admin.from('eduset_settings').select('access_token_enc').eq('id', 1).maybeSingle();
  if (!settings?.access_token_enc) {
    return Response.json({ message: '관리자가 아직 자습신청 토큰을 등록하지 않았습니다.' }, { status: 503 });
  }

  let edusetToken: string;
  try {
    edusetToken = decryptToken(settings.access_token_enc, edusetTokenSecret);
  } catch (err) {
    return Response.json({ message: '토큰 복호화 실패: ' + (err as Error).message }, { status: 500 });
  }

  const dateStr = localDateStrKST();
  let appliedIds: Set<string>;
  try {
    appliedIds = await getAppliedStudentIds(edusetToken, dateStr);
  } catch (err) {
    return Response.json({ message: '자습신청 조회 실패: ' + (err as Error).message }, { status: 502 });
  }

  // profiles(id) <- device_tokens.user_id FK lets PostgREST embed this automatically.
  const { data: profiles, error: profilesError } = await admin
    .from('profiles')
    .select('id, student_id, device_tokens(token, platform)')
    .not('student_id', 'is', null)
    .neq('is_approved', false)
    .eq('notify_selfstudy_reminder', true);

  if (profilesError) {
    return Response.json({ message: '프로필 조회 실패: ' + profilesError.message }, { status: 500 });
  }

  const serviceAccount = JSON.parse(firebaseServiceAccountRaw);
  const accessToken = await getFcmAccessToken(serviceAccount);

  let notified = 0;
  let failed = 0;
  for (const profile of profiles ?? []) {
    if (!profile.student_id || appliedIds.has(profile.student_id)) continue;

    const tokens = ((profile.device_tokens ?? []) as { token: string; platform: string }[])
      .filter((t) => t.platform === 'android')
      .map((t) => t.token);

    for (const deviceToken of tokens) {
      try {
        await sendFcmPush(serviceAccount.project_id, accessToken, deviceToken, 'CBGPU', '아직 오늘 자습신청을 안 하셨어요. 지금 신청해주세요!');
        notified++;
      } catch {
        failed++;
      }
    }
  }

  return Response.json({ ok: true, date: dateStr, checked: profiles?.length ?? 0, notified, failed });
});
