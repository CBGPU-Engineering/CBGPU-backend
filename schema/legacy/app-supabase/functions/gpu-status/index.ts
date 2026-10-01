// Mirrors src/app/api/gpu-status/route.ts. 주소는 gpu_endpoints 테이블이 가지고, 읽지 못할 때만 아래 폴백을 쓴다.
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const FALLBACK_GPUS = [
  { id: 1, url: 'https://tonight-rep-admit-cooperation.trycloudflare.com' },
  { id: 2, url: 'https://nearest-situations-elite-tools.trycloudflare.com' },
  { id: 3, url: 'https://expressions-invention-investments-consortium.trycloudflare.com' },
  { id: 4, url: 'https://september-notification-pants-whatever.trycloudflare.com' },
];

async function loadGpus(): Promise<{ id: number; url: string }[]> {
  try {
    const base = Deno.env.get('SUPABASE_URL');
    const key = Deno.env.get('SUPABASE_ANON_KEY');
    if (!base || !key) return FALLBACK_GPUS;
    const response = await fetch(`${base}/rest/v1/gpu_endpoints?select=gpu_id,public_url&order=gpu_id`, {
      headers: { apikey: key, Authorization: `Bearer ${key}` },
    });
    if (!response.ok) return FALLBACK_GPUS;
    const rows: { gpu_id: number; public_url: string }[] = await response.json();
    return FALLBACK_GPUS.map(fallback => {
      const row = rows.find(item => item.gpu_id === fallback.id);
      return row ? { id: row.gpu_id, url: row.public_url } : fallback;
    });
  } catch {
    return FALLBACK_GPUS;
  }
}

async function ping(url: string): Promise<'online' | 'offline'> {
  try {
    const ac = new AbortController();
    const t = setTimeout(() => ac.abort(), 5000);
    const r = await fetch(url, { method: 'GET', signal: ac.signal, redirect: 'follow' });
    clearTimeout(t);
    return r.ok ? 'online' : 'offline';
  } catch {
    return 'offline';
  }
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const results = await Promise.all((await loadGpus()).map(async (g) => ({ id: g.id, status: await ping(g.url), url: g.url })));
  return Response.json({ gpus: results, checkedAt: new Date().toISOString() }, { headers: corsHeaders });
});
