// Ported verbatim from src/app/api/gpu-status/route.ts.
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const GPUS = [
  { id: 1, url: 'https://bbb1f73aa6bd31.lhr.life' },
  { id: 2, url: 'https://109a33522c2e35.lhr.life' },
  { id: 3, url: 'https://ccb4e81a1e065b.lhr.life' },
  { id: 4, url: 'https://d145edf2d1b8f1.lhr.life' },
];

async function ping(url: string): Promise<'online' | 'offline'> {
  try {
    const ac = new AbortController();
    const t = setTimeout(() => ac.abort(), 5000);
    const r = await fetch(url, { method: 'HEAD', signal: ac.signal, redirect: 'follow' });
    clearTimeout(t);
    return r.status < 500 ? 'online' : 'offline';
  } catch {
    return 'offline';
  }
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const results = await Promise.all(GPUS.map(async (g) => ({ id: g.id, status: await ping(g.url), url: g.url })));
  return Response.json({ gpus: results, checkedAt: new Date().toISOString() }, { headers: corsHeaders });
});
