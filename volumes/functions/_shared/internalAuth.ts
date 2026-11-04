// These notify-* functions are only ever meant to be called from our own DB
// triggers (via pg_net — see supabase/notify_triggers_migration.sql), never
// directly by a client. Deployed with --no-verify-jwt like the rest, so this
// check is the only thing standing between the function and the public
// internet. Uses a dedicated secret (INTERNAL_TRIGGER_SECRET) rather than
// SUPABASE_SERVICE_ROLE_KEY: this project has both a legacy JWT-format
// service_role key and a newer sb_secret_* key, and which one Supabase
// auto-injects into an Edge Function's env is not something worth depending
// on — a secret only we set and only we embed in the trigger SQL sidesteps
// that entirely.
export function requireServiceRole(req: Request): boolean {
  const token = req.headers.get('authorization')?.replace(/^Bearer\s+/i, '');
  const expected = Deno.env.get('INTERNAL_TRIGGER_SECRET');
  return !!token && !!expected && token === expected;
}
