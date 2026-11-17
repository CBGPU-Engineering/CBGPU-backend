// Usage statistics for the developer portal: gateway request counts (from the Supabase gateway access log)
// and account activity. Only aggregates leave this module: no paths, query strings, addresses or user ids.
const SERVICES: [string, string][] = [['rest', '/rest/v1'], ['auth', '/auth/v1'], ['storage', '/storage/v1'], ['realtime', '/realtime/v1'], ['functions', '/functions/v1']];
const MONTHS: Record<string, number> = {Jan: 0, Feb: 1, Mar: 2, Apr: 3, May: 4, Jun: 5, Jul: 6, Aug: 7, Sep: 8, Oct: 9, Nov: 10, Dec: 11};
const HOUR = 3600_000;
// 172.18.0.1 - - [17/Nov/2026:11:24:36 +0000] "GET /rest/v1/x?y=1 HTTP/1.1" 200 10275 "-" "node"
const LINE = /\[(\d{1,2})\/([A-Za-z]{3})\/(\d{4}):(\d{2}):(\d{2}):(\d{2}) ([+-])(\d{2})(\d{2})\] "[A-Z]+ (\S+)[^"]*" (\d{3}) \S+ "[^"]*" "([^"]*)"/;
// Browsers and the Android/Flutter app count as people; everything else (Next server, workers, edge functions) is internal.
const CLIENT_AGENT = /Mozilla|Dart|Dalvik|okhttp/;

export type ApiUsage = {
  requests_24h: number; requests_1h: number; client_requests_24h: number; client_requests_1h: number;
  server_errors_24h: number; error_rate: number | null;
  per_hour: {hour_start: string; requests: number; client_requests: number; errors: number}[];
  services: Record<string, number>;
};

export function parseGatewayLog(text: string, now = Date.now()): ApiUsage {
  const start = Math.floor(now / HOUR) * HOUR - 23 * HOUR;
  const perHour = Array.from({length: 24}, (_, i) => ({hour_start: new Date(start + i * HOUR).toISOString(), requests: 0, client_requests: 0, errors: 0}));
  const services: Record<string, number> = {rest: 0, auth: 0, storage: 0, realtime: 0, functions: 0, other: 0};
  let total = 0, errors = 0, lastHour = 0, clients = 0, clientsLastHour = 0;
  for (const line of text.split('\n')) {
    const m = LINE.exec(line);
    if (!m) continue;
    const month = MONTHS[m[2]];
    if (month === undefined) continue;
    const offset = (m[7] === '-' ? -1 : 1) * (Number(m[8]) * 60 + Number(m[9])) * 60_000;
    const at = Date.UTC(Number(m[3]), month, Number(m[1]), Number(m[4]), Number(m[5]), Number(m[6])) - offset;
    if (at < start || at > now + 60_000) continue;
    const bucket = perHour[Math.min(23, Math.floor((at - start) / HOUR))];
    const status = Number(m[11]), path = m[10].split('?')[0];
    bucket.requests++; total++;
    const client = CLIENT_AGENT.test(m[12]);
    if (client) {bucket.client_requests++; clients++}
    if (at > now - HOUR) {lastHour++; if (client) clientsLastHour++}
    if (status >= 500) {bucket.errors++; errors++}
    services[(SERVICES.find(([, prefix]) => path === prefix || path.startsWith(prefix + '/')) || ['other'])[0]]++;
  }
  return {requests_24h: total, requests_1h: lastHour, client_requests_24h: clients, client_requests_1h: clientsLastHour, server_errors_24h: errors, error_rate: total ? Math.round(errors / total * 10000) / 100 : null, per_hour: perHour, services};
}

export type AccountUsage = {total: number; dau: number; wau: number; mau: number; new_today: number; new_7d: number; logins_today: number; logins_7d: number; logins_30d: number};

/** SQL that returns one JSON row for the account statistics (signed-in activity by session refresh time). */
export const ACCOUNT_USAGE_SQL = `SELECT json_build_object(
 'total',(SELECT count(*) FROM auth.users),
 'dau',(SELECT count(DISTINCT user_id) FROM auth.sessions WHERE coalesce(refreshed_at,updated_at,created_at)>now()-interval '1 day'),
 'wau',(SELECT count(DISTINCT user_id) FROM auth.sessions WHERE coalesce(refreshed_at,updated_at,created_at)>now()-interval '7 days'),
 'mau',(SELECT count(DISTINCT user_id) FROM auth.sessions WHERE coalesce(refreshed_at,updated_at,created_at)>now()-interval '30 days'),
 'new_today',(SELECT count(*) FROM auth.users WHERE created_at>now()-interval '1 day'),
 'new_7d',(SELECT count(*) FROM auth.users WHERE created_at>now()-interval '7 days'),
 'logins_today',(SELECT count(*) FROM auth.audit_log_entries WHERE payload->>'action'='login' AND created_at>now()-interval '1 day'),
 'logins_7d',(SELECT count(*) FROM auth.audit_log_entries WHERE payload->>'action'='login' AND created_at>now()-interval '7 days'),
 'logins_30d',(SELECT count(*) FROM auth.audit_log_entries WHERE payload->>'action'='login' AND created_at>now()-interval '30 days'))`;

export function parseAccountUsage(output: string): AccountUsage | null {
  try {
    const v = JSON.parse(output.trim());
    const keys = ['total', 'dau', 'wau', 'mau', 'new_today', 'new_7d', 'logins_today', 'logins_7d', 'logins_30d'] as const;
    if (!keys.every(key => Number.isFinite(Number(v[key])))) return null;
    return Object.fromEntries(keys.map(key => [key, Number(v[key])])) as AccountUsage;
  } catch { return null }
}
