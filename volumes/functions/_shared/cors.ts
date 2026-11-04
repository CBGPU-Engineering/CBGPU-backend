// Shared CORS headers for every Edge Function the Flutter app calls directly
// from a browser (Flutter Web). Native builds aren't subject to CORS at all,
// but web is, and unlike the old Next.js routes these functions are called
// from arbitrary origins (no same-origin default), so every response —
// including the OPTIONS preflight — needs these on it.
export const corsHeaders = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type, Authorization, x-cbgpu-auth, x-jwt',
};

export function withCors(body: unknown, init?: ResponseInit): Response {
  return Response.json(body, { ...init, headers: { ...corsHeaders, ...(init?.headers ?? {}) } });
}

export function handleOptions(req: Request): Response | null {
  if (req.method === 'OPTIONS') return new Response('ok', { headers: corsHeaders });
  return null;
}
