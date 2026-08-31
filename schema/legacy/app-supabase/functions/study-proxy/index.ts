// Ported from src/app/api/study/proxy/[...path]/route.ts. Every caller,
// regardless of identity, is served through one admin-registered shared
// eduset token (see getSharedToken below) — this route only gatekeeps who
// may use that token and which upstream paths are reachable through it.
import { createClient } from 'npm:@supabase/supabase-js@2.112.4';
import crypto from 'node:crypto';
import { Buffer } from 'node:buffer';
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const BASE_URL = 'https://api.cbsh.edu-set.com';
const supabaseUrl = Deno.env.get('SUPABASE_URL')!;
const anonKey = Deno.env.get('SUPABASE_ANON_KEY')!;
const supabase = createClient(supabaseUrl, anonKey);

// Same AES-256-GCM scheme as src/lib/edusetTokenCrypto.ts in the Next.js app.
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

const ID = '[^/]+';
const ALLOWED_ROUTES: { method: string; pattern: RegExp }[] = [
  { method: 'GET', pattern: /^\/rooms$/ },
  { method: 'GET', pattern: new RegExp(`^/rooms/${ID}/composition$`) },
  { method: 'GET', pattern: new RegExp(`^/rooms/${ID}/seats/occupations$`) },
  { method: 'GET', pattern: /^\/students$/ },
  { method: 'GET', pattern: new RegExp(`^/students/${ID}$`) },
  { method: 'GET', pattern: new RegExp(`^/users/${ID}$`) },
  { method: 'GET', pattern: /^\/policies\/selfstudy$/ },
  { method: 'POST', pattern: /^\/selfstudy\/search$/ },
];

// Strips the function's own routing prefix (/functions/v1/study-proxy locally
// it's just /study-proxy) so the remainder matches the ALLOWED_ROUTES table
// exactly the way the Next.js catch-all route's path did.
function getProxyPathname(url: URL): string {
  const pathname = url.pathname.replace(/^\/(functions\/v1\/)?study-proxy\/?/, '/');
  return pathname === '/' ? '' : pathname;
}

async function requireApprovedUser(req: Request): Promise<boolean> {
  const token = req.headers.get('x-cbgpu-auth')?.replace(/^Bearer\s+/i, '');
  if (!token) return false;

  const client = createClient(supabaseUrl, anonKey, { global: { headers: { Authorization: `Bearer ${token}` } } });
  const { data: { user }, error } = await client.auth.getUser(token);
  if (error || !user) return false;

  const { data: profile } = await client.from('profiles').select('is_approved').eq('id', user.id).maybeSingle();
  return profile?.is_approved !== false;
}

let cachedToken: { value: string | null; fetchedAt: number } | null = null;
const TOKEN_CACHE_TTL_MS = 60_000;

function getResponseContentType(headers: Headers): string {
  // Duplicate upstream Content-Type headers are comma-joined by Fetch. Passing
  // that joined value to Response produces "Invalid media type" in Deno, so
  // forward only the first complete media type.
  const value = headers.get('content-type');
  return value?.split(',')[0].trim() || 'application/json';
}

async function getSharedToken(): Promise<string | null> {
  if (cachedToken && Date.now() - cachedToken.fetchedAt < TOKEN_CACHE_TTL_MS) return cachedToken.value;

  const { data } = await supabase.from('eduset_settings').select('access_token_enc').eq('id', 1).maybeSingle();
  let value: string | null = null;
  if (data?.access_token_enc) {
    const secret = Deno.env.get('EDUSET_TOKEN_SECRET');
    try {
      value = secret ? decryptToken(data.access_token_enc, secret) : null;
    } catch {
      value = null;
    }
  }
  cachedToken = { value, fetchedAt: Date.now() };
  return value;
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const url = new URL(req.url);
  const pathname = getProxyPathname(url);
  const allowed = ALLOWED_ROUTES.some((r) => r.method === req.method && r.pattern.test(pathname));
  if (!allowed) {
    return Response.json({ message: '지원하지 않는 요청입니다.' }, { status: 403, headers: corsHeaders });
  }

  if (!(await requireApprovedUser(req))) {
    return Response.json({ message: '로그인 후 승인된 계정만 조회할 수 있습니다.' }, { status: 401, headers: corsHeaders });
  }

  const targetUrl = new URL(`${pathname}${url.search}`, BASE_URL);
  const token = await getSharedToken();
  if (!token) {
    return Response.json({ message: '관리자가 아직 자습신청 토큰을 등록하지 않았습니다.' }, { status: 503, headers: corsHeaders });
  }

  const body = req.method === 'GET' ? undefined : await req.text();

  const upstream = await fetch(targetUrl, {
    method: req.method,
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}`, 'x-jwt': token },
    body,
  });

  const responseBody = await upstream.text();
  return new Response(responseBody, {
    status: upstream.status,
    headers: { ...corsHeaders, 'Content-Type': getResponseContentType(upstream.headers), 'Cache-Control': 'no-store' },
  });
});
