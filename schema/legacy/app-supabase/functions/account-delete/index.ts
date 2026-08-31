// Ported from src/app/api/account/delete/route.ts — resolves the target
// user id ONLY from the caller's own bearer token, never a body-supplied id.
import { createClient } from 'npm:@supabase/supabase-js@2.112.4';
import { corsHeaders, handleOptions } from '../_shared/cors.ts';

const supabaseUrl = Deno.env.get('SUPABASE_URL')!;
const anonKey = Deno.env.get('SUPABASE_ANON_KEY')!;
const serviceRoleKey = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY')!;

async function getRequestingUserId(req: Request): Promise<string | null> {
  const token = req.headers.get('authorization')?.replace(/^Bearer\s+/i, '');
  if (!token) return null;
  const client = createClient(supabaseUrl, anonKey);
  const { data: { user }, error } = await client.auth.getUser(token);
  if (error || !user) return null;
  return user.id;
}

Deno.serve(async (req) => {
  const preflight = handleOptions(req);
  if (preflight) return preflight;

  const userId = await getRequestingUserId(req);
  if (!userId) {
    return Response.json({ message: '로그인이 필요합니다.' }, { status: 401, headers: corsHeaders });
  }

  const admin = createClient(supabaseUrl, serviceRoleKey, { auth: { autoRefreshToken: false, persistSession: false } });

  const { error: profileError } = await admin
    .from('profiles')
    .update({ full_name: null, student_id: null, avatar_url: null, is_approved: false })
    .eq('id', userId);
  if (profileError) {
    return Response.json({ message: '프로필 삭제 실패: ' + profileError.message }, { status: 500, headers: corsHeaders });
  }

  const { error: deleteError } = await admin.auth.admin.deleteUser(userId, true);
  if (deleteError) {
    return Response.json({ message: '계정 삭제 실패: ' + deleteError.message }, { status: 500, headers: corsHeaders });
  }

  return Response.json({ ok: true }, { headers: corsHeaders });
});
