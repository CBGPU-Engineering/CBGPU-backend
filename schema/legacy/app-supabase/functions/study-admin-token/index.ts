// Ported from src/app/api/study/admin-token/route.ts — admin-only shared
// eduset token management. Uses the caller's own bearer token through RLS
// (not the service role) exactly like the original, so the existing
// eduset_settings admin-write policy is what actually gates the upsert.
import { createClient } from 'npm:@supabase/supabase-js@2.112.4';
import crypto from 'node:crypto';
import { Buffer } from 'node:buffer';
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const EDUSET_BASE_URL = 'https://api.cbsh.edu-set.com';
const supabaseUrl = Deno.env.get('SUPABASE_URL')!;
const anonKey = Deno.env.get('SUPABASE_ANON_KEY')!;

// Same AES-256-GCM scheme as src/lib/edusetTokenCrypto.ts in the Next.js app.
function encryptToken(plain: string, secret: string): string {
  const key = crypto.createHash('sha256').update(secret).digest();
  const iv = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv('aes-256-gcm', key, iv);
  const encrypted = Buffer.concat([cipher.update(plain, 'utf8'), cipher.final()]);
  const tag = cipher.getAuthTag();
  return Buffer.concat([iv, tag, encrypted]).toString('base64');
}

async function requireAdmin(req: Request) {
  const token = req.headers.get('authorization')?.replace(/^Bearer\s+/i, '');
  if (!token) return null;

  const client = createClient(supabaseUrl, anonKey, { global: { headers: { Authorization: `Bearer ${token}` } } });
  const { data: { user }, error } = await client.auth.getUser(token);
  if (error || !user) return null;

  const { data: profile } = await client.from('profiles').select('is_admin').eq('id', user.id).single();
  return profile?.is_admin ? { user, client } : null;
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const admin = await requireAdmin(req);
  if (!admin) {
    return Response.json({ message: '관리자만 접근할 수 있습니다.' }, { status: 403, headers: corsHeaders });
  }

  if (req.method === 'GET') {
    const { data } = await admin.client.from('eduset_settings').select('updated_at').eq('id', 1).maybeSingle();
    return Response.json({ updatedAt: data?.updated_at ?? null }, { headers: corsHeaders });
  }

  // deno-lint-ignore no-explicit-any
  const body = await req.json().catch(() => ({}) as any);
  const accessToken = String(body?.accessToken ?? '').trim();
  if (!accessToken) {
    return Response.json({ message: 'accessToken을 입력해주세요.' }, { status: 400, headers: corsHeaders });
  }

  const verify = await fetch(`${EDUSET_BASE_URL}/rooms`, { headers: { Authorization: `Bearer ${accessToken}` } });
  if (!verify.ok) {
    return Response.json({ message: '자습신청 사이트에서 토큰을 거절했습니다. 공식 사이트에서 다시 로그인해 새 토큰을 받아주세요.' }, { status: 400, headers: corsHeaders });
  }

  const secret = Deno.env.get('EDUSET_TOKEN_SECRET');
  if (!secret) {
    return Response.json({ message: 'EDUSET_TOKEN_SECRET secret이 설정되어 있지 않습니다.' }, { status: 500, headers: corsHeaders });
  }

  let encrypted: string;
  try {
    encrypted = encryptToken(accessToken, secret);
  } catch (err) {
    return Response.json({ message: (err as Error).message ?? '토큰 암호화에 실패했습니다.' }, { status: 500, headers: corsHeaders });
  }

  const { error } = await admin.client.from('eduset_settings').upsert({ id: 1, access_token_enc: encrypted, updated_at: new Date().toISOString(), updated_by: admin.user.id });
  if (error) {
    return Response.json({ message: `저장 중 오류가 발생했습니다: ${error.message}` }, { status: 500, headers: corsHeaders });
  }

  return Response.json({ ok: true }, { headers: corsHeaders });
});
