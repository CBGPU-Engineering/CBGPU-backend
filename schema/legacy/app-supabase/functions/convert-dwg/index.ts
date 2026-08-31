import { createClient } from 'npm:@supabase/supabase-js@2.112.4';
import { readDwg, writeDxf } from 'npm:nasjidwg@0.15.1';
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const maxBytes = 8 * 1024 * 1024;

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;
  if (req.method !== 'POST') return Response.json({ error: 'Method not allowed' }, { status: 405, headers: corsHeaders });

  const authorization = req.headers.get('Authorization');
  if (!authorization) return Response.json({ error: '로그인이 필요합니다.' }, { status: 401, headers: corsHeaders });
  const client = createClient(Deno.env.get('SUPABASE_URL')!, Deno.env.get('SUPABASE_ANON_KEY')!, {
    global: { headers: { Authorization: authorization } },
  });
  const { data: { user }, error } = await client.auth.getUser();
  if (error || !user) return Response.json({ error: '유효한 로그인이 필요합니다.' }, { status: 401, headers: corsHeaders });

  const declaredLength = Number(req.headers.get('content-length') ?? 0);
  if (declaredLength > maxBytes) return Response.json({ error: 'DWG 파일은 8MB 이하여야 합니다.' }, { status: 413, headers: corsHeaders });
  const bytes = new Uint8Array(await req.arrayBuffer());
  if (bytes.length === 0 || bytes.length > maxBytes) {
    return Response.json({ error: bytes.length === 0 ? 'DWG 파일이 비어 있습니다.' : 'DWG 파일은 8MB 이하여야 합니다.' }, { status: 400, headers: corsHeaders });
  }

  try {
    const drawing = readDwg(bytes);
    const dxf = writeDxf(drawing);
    return new Response(dxf, { headers: { ...corsHeaders, 'Content-Type': 'application/dxf', 'Cache-Control': 'no-store' } });
  } catch (conversionError) {
    console.error('DWG conversion failed', conversionError);
    return Response.json({ error: 'DWG 파일을 읽을 수 없습니다.' }, { status: 422, headers: corsHeaders });
  }
});
