// Stores the same notification in the person's inbox (public.notifications) at the moment a push is decided, so the
// inbox is complete even when the app was closed, the account has no registered device, or the push fails.
// Best effort by design: a failed inbox write must never block or fail the push itself.
export type InboxNotification = {
  user_id: string; kind: string; title: string; body?: string;
  route?: string | null; data?: Record<string, string>; dedupe_key?: string;
};
type Deps = { url?: string; key?: string; fetch?: typeof fetch };

const KIND = /^[a-z0-9_.-]{1,40}$/;
// An app path ("/study") or one of our own https hosts; never "//host" or "/\host", which browsers treat as another site.
const ROUTE = /^(\/([^/\\]|$)|https:\/\/[a-z0-9.-]+\.cbgpu\.com\/)/;

/** Fits the table's check constraints, or null when the item cannot be stored. */
export function inboxRow(n: InboxNotification) {
  if (!/^[0-9a-f-]{36}$/i.test(n.user_id) || !KIND.test(n.kind) || !n.title?.trim()) return null;
  const data = n.data && JSON.stringify(n.data).length <= 2000 ? n.data : {};
  return {
    p_user_id: n.user_id, p_kind: n.kind, p_title: n.title.trim().slice(0, 120), p_body: (n.body ?? '').slice(0, 700),
    p_route: n.route && n.route.length <= 300 && ROUTE.test(n.route) ? n.route : null,
    p_data: data, p_dedupe_key: n.dedupe_key ? n.dedupe_key.slice(0, 200) : null,
  };
}

/** Returns how many were stored (a repeated dedupe_key counts as stored). Never throws. */
export async function recordNotifications(items: InboxNotification[], deps: Deps = {}): Promise<number> {
  // deno-lint-ignore no-explicit-any
  const env = (globalThis as any).Deno?.env;
  const url = deps.url ?? env?.get('SUPABASE_URL'), key = deps.key ?? env?.get('SUPABASE_SERVICE_ROLE_KEY');
  if (!url || !key) return 0;
  const send = deps.fetch ?? fetch;
  const rows = items.map(inboxRow).filter((row): row is NonNullable<ReturnType<typeof inboxRow>> => row !== null);
  const results = await Promise.allSettled(rows.map(async row => {
    const response = await send(`${url}/rest/v1/rpc/cbgpu_notify`, {
      method: 'POST', headers: { apikey: key, Authorization: `Bearer ${key}`, 'Content-Type': 'application/json' }, body: JSON.stringify(row),
    });
    if (!response.ok) throw new Error(`inbox ${response.status}`);
  }));
  return results.filter(result => result.status === 'fulfilled').length;
}
