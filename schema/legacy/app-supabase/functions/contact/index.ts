// Ported from src/app/api/contact/route.ts — same Resend send, same
// best-effort per-instance rate limit (a warm Edge Function instance keeps
// this Map alive between invocations the same way a warm Vercel lambda did;
// neither guarantees a shared limit across instances, both accepted that).
import { Resend } from 'npm:resend@4.0.1';
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const resend = new Resend(Deno.env.get('RESEND_API_KEY'));
const RECIPIENTS = ['lmw.hpc@gmail.com', 'ahn.hyunjun2009@gmail.com'];
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
    from: 'CBGPU <cbgpu@airhood.dev>',
    to: RECIPIENTS,
    replyTo: email,
    subject: `[CBGPU 문의] ${category ? `[${escHtml(category)}] ` : ''}${escHtml(name)}`,
    html: `
      <div style="font-family: monospace; max-width: 600px; margin: 0 auto; padding: 24px; border: 2px solid #e5e7eb; border-radius: 12px;">
        <h2 style="font-size: 20px; font-weight: 900; letter-spacing: -0.5px; margin: 0 0 20px;">CBGPU 문의</h2>
        <table style="width: 100%; border-collapse: collapse; font-size: 13px;">
          <tr style="border-bottom: 1px solid #e5e7eb;">
            <td style="padding: 10px 0; font-weight: 700; color: #6b7280; width: 80px;">이름</td>
            <td style="padding: 10px 0; font-weight: 600;">${escHtml(name)}</td>
          </tr>
          <tr style="border-bottom: 1px solid #e5e7eb;">
            <td style="padding: 10px 0; font-weight: 700; color: #6b7280;">이메일</td>
            <td style="padding: 10px 0; font-weight: 600;">${escHtml(email)}</td>
          </tr>
          <tr style="border-bottom: 1px solid #e5e7eb;">
            <td style="padding: 10px 0; font-weight: 700; color: #6b7280;">분류</td>
            <td style="padding: 10px 0; font-weight: 600;">${escHtml(category || '기타')}</td>
          </tr>
          <tr>
            <td style="padding: 10px 0; font-weight: 700; color: #6b7280; vertical-align: top;">내용</td>
            <td style="padding: 10px 0; font-weight: 600; white-space: pre-wrap;">${escHtml(message)}</td>
          </tr>
        </table>
      </div>
    `,
  });

  if (error) {
    return Response.json({ error: error.message }, { status: 500, headers: corsHeaders });
  }
  return Response.json({ ok: true }, { headers: corsHeaders });
});
