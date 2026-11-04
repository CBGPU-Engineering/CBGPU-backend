import { emailSender, SUPPORT_EMAIL, renderEmail } from '../_shared/email.ts';
// Ported from src/app/api/contact/route.ts — same Resend send, same
// best-effort per-instance rate limit (a warm Edge Function instance keeps
// this Map alive between invocations the same way a warm Vercel lambda did;
// neither guarantees a shared limit across instances, both accepted that).
import { Resend } from 'npm:resend@4.0.1';
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const resend = new Resend(Deno.env.get('RESEND_API_KEY'));
const RECIPIENTS = [SUPPORT_EMAIL];
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

function escHtml(s: string) {
  return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

const rateLimitHits = new Map<string, number[]>();
function isRateLimited(ip: string): boolean {
  const now = Date.now();
  const windowMs = 10 * 60 * 1000;
  const hits = (rateLimitHits.get(ip) ?? []).filter((t) => now - t < windowMs);
  hits.push(now);
  rateLimitHits.set(ip, hits);
  return hits.length > 5;
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const ip = req.headers.get('x-forwarded-for')?.split(',')[0]?.trim() || 'unknown';
  if (isRateLimited(ip)) {
    return Response.json({ error: '잠시 후 다시 시도해 주세요.' }, { status: 429, headers: corsHeaders });
  }

  // deno-lint-ignore no-explicit-any
  const { name, email, category, message } = await req.json().catch(() => ({}) as any);

  if (!name || !email || !message || typeof name !== 'string' || typeof email !== 'string' || typeof message !== 'string') {
    return Response.json({ error: '필수 항목을 입력해 주세요.' }, { status: 400, headers: corsHeaders });
  }
  if (!EMAIL_RE.test(email) || name.length > 100 || message.length > 5000 || (category && String(category).length > 50)) {
    return Response.json({ error: '입력값이 올바르지 않습니다.' }, { status: 400, headers: corsHeaders });
  }

  const { error } = await resend.emails.send({
    from: emailSender(),
    to: RECIPIENTS,
    replyTo: email,
    subject: `[CBGPU 문의] ${category ? `[${escHtml(category)}] ` : ''}${escHtml(name)}`,
    text: `CBGPU 문의\n이름: ${name}\n이메일: ${email}\n분류: ${category || '기타'}\n\n${message}`,
    html: renderEmail({
      title: '새 문의가 도착했어요',
      preview: 'CBGPU 문의: ' + String(category || '기타'),
      description: '웹사이트에서 접수된 문의입니다. 아래 내용을 확인하고 문의자에게 답변해 주세요.',
      bodyHtml: `<table role="presentation" width="100%" style="font-size:14px;line-height:1.8;border-collapse:collapse;"><tr><td style="padding:8px 0;color:#6b7280;width:70px;">이름</td><td>${escHtml(name)}</td></tr><tr><td style="padding:8px 0;color:#6b7280;">이메일</td><td>${escHtml(email)}</td></tr><tr><td style="padding:8px 0;color:#6b7280;">분류</td><td>${escHtml(String(category || '기타'))}</td></tr></table><div style="padding:18px;margin-top:18px;border:1px solid #e4e7ec;border-radius:12px;background:#f8faff;white-space:pre-wrap;font-size:14px;line-height:1.8;overflow-wrap:anywhere;">${escHtml(message)}</div>`,
      buttonLabel: '문의자에게 답장하기',
      buttonUrl: 'mailto:' + encodeURIComponent(email),
      note: '이 메일에 바로 답장해도 문의자에게 전달됩니다.',
    }),
  });

  if (error) {
    return Response.json({ error: error.message }, { status: 500, headers: corsHeaders });
  }
  return Response.json({ ok: true }, { headers: corsHeaders });
});
