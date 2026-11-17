import test from 'node:test';
import assert from 'node:assert/strict';
import worker, {PUBLIC_COMPONENTS, buildPublicStatus, recordSample, renderStatusPage, uptime} from './worker.mts';

const NOW = Date.parse('2026-11-17T10:05:00.000Z');
const allUp = {website: true, admin: true, developers: true, backend: true, docs: true, preprints: true, gpu1: true, gpu2: true, gpu3: true, gpu4: true, offsite_backup: true};
const fakeEnv = (state: unknown, puts: string[] = []) => ({
  OPS_KEY: 'secret-ops-key', MONITOR_KEY: 'secret-monitor-key', RESEND_KEY: 'secret-resend', RECIPIENTS: '["a@example.invalid"]',
  OPS: {get: async (key: string) => key === 'monitor' ? state : key === 'backups' ? [{id: '20261117T000000Z', created_at: '2026-11-17T00:00:00Z'}] : null, put: async (key: string, value: string) => { puts.push(key + ':' + value) }},
});

test('recordSample counts only public components, one sample per call, per KST day', () => {
  let history: any = recordSample(undefined, {...allUp, gpu2: false}, '2026-11-17T10:00:00.000Z');
  history = recordSample(history, allUp, '2026-11-17T10:05:00.000Z');
  assert.deepEqual(history['2026-11-17'].gpu2, [1, 2]);
  assert.deepEqual(history['2026-11-17'].website, [2, 2]);
  assert.equal(history['2026-11-17'].offsite_backup, undefined);
  // 15:00 UTC is already the next day in Korea.
  history = recordSample(history, allUp, '2026-11-17T15:00:00.000Z');
  assert.ok(history['2026-11-18']);
});

test('history keeps at most 90 days', () => {
  let history: any;
  for (let i = 0; i < 100; i++) history = recordSample(history, allUp, new Date(NOW - (99 - i) * 86400000).toISOString());
  assert.equal(Object.keys(history).length, 90);
});

test('uptime is a percentage of samples, null without samples', () => {
  const history = {'2026-11-17': {website: [3, 4]}};
  assert.equal(uptime(history, 'website', 30, NOW), 75);
  assert.equal(uptime(history, 'admin', 30, NOW), null);
  assert.equal(uptime({}, null, 30, NOW), null);
});

test('overall status: operational, degraded, outage, unknown', () => {
  const fresh = new Date(NOW - 60000).toISOString();
  assert.equal(buildPublicStatus({checked_at: fresh, checks: allUp, incident: false}, NOW).overall, 'operational');
  assert.equal(buildPublicStatus({checked_at: fresh, checks: {...allUp, docs: false}, incident: false}, NOW).overall, 'degraded');
  assert.equal(buildPublicStatus({checked_at: fresh, checks: {...allUp, website: false}, incident: true}, NOW).overall, 'outage');
  assert.equal(buildPublicStatus({checked_at: new Date(NOW - 11 * 60000).toISOString(), checks: allUp}, NOW).overall, 'unknown');
  assert.equal(buildPublicStatus(null, NOW).overall, 'unknown');
  assert.equal(buildPublicStatus({checked_at: new Date(NOW - 11 * 60000).toISOString(), checks: allUp}, NOW).components.every(c => c.up === null), true);
});

test('public output never contains internal fields, URLs or secrets', () => {
  const state = {checked_at: new Date(NOW - 1000).toISOString(), checks: allUp, failures: 2, successes: 0, incident: false, incident_id: 123, alert_error: 'Email provider rejected the alert', last_alert_at: 'x', history: recordSample(undefined, allUp, new Date(NOW).toISOString())};
  const status = buildPublicStatus(state, NOW);
  const text = JSON.stringify(status) + renderStatusPage(status);
  for (const forbidden of ['cbgpu.com', 'Bearer', 'failures', 'alert_error', 'incident_id', 'offsite', 'backup', 'secret', 'http://', 'https://']) assert.ok(!text.includes(forbidden), 'leaked: ' + forbidden);
  assert.deepEqual(status.components.map(c => c.id), PUBLIC_COMPONENTS.map(([id]) => id));
  assert.ok(!/<script/i.test(renderStatusPage(status)));
});

test('GET / and /api/status are public; key-protected routes stay protected', async () => {
  const state = {checked_at: new Date().toISOString(), checks: allUp, incident: false, history: {}};
  const env = fakeEnv(state);
  const page = await worker.fetch(new Request('https://status.example.invalid/'), env);
  assert.equal(page.status, 200);
  assert.match(page.headers.get('content-type') || '', /text\/html/);
  assert.match(page.headers.get('content-security-policy') || '', /default-src 'none'/);
  const api = await worker.fetch(new Request('https://status.example.invalid/api/status'), env);
  assert.equal(api.status, 200);
  const body = await api.json();
  assert.equal(body.overall, 'operational');
  assert.ok(!JSON.stringify(body).includes('backups'));
  for (const path of ['/status', '/backups/20261117T000000Z']) assert.notEqual((await worker.fetch(new Request('https://status.example.invalid' + path), env)).status, 200);
  assert.notEqual((await worker.fetch(new Request('https://status.example.invalid/', {method: 'PUT', body: 'x'}), env)).status, 200);
  assert.equal((await worker.fetch(new Request('https://status.example.invalid/health'), env)).status, 200);
});

test('scheduled check samples history only on 5-minute marks', async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = (async () => new Response('ok', {status: 200})) as typeof fetch;
  try {
    for (const [iso, expectHistory] of [['2026-11-17T10:05:20.000Z', true], ['2026-11-17T10:07:20.000Z', false]] as const) {
      const realDate = Date, fixed = realDate.parse(iso);
      class FakeDate extends realDate { constructor(value?: any) { super(value === undefined ? fixed : value) } static now() { return fixed } }
      (globalThis as any).Date = FakeDate;
      const puts: string[] = [];
      let pending: Promise<unknown> = Promise.resolve();
      try { await worker.scheduled({}, fakeEnv({failures: 0, successes: 0, incident: false}, puts), {waitUntil: (p: Promise<unknown>) => { pending = p }}); await pending } finally { (globalThis as any).Date = realDate }
      const saved = puts.map(p => p.replace(/^monitor:/, '')).map(p => { try { return JSON.parse(p) } catch { return null } }).find(Boolean);
      if (expectHistory) { assert.ok(saved?.history?.['2026-11-17'], 'history missing'); assert.deepEqual(saved.history['2026-11-17'].website, [1, 1]) }
      else assert.equal(saved?.history, undefined);
    }
  } finally { globalThis.fetch = realFetch }
});

test('GPU gateway 401 (signed-out visitor) counts as healthy, other 4xx/5xx do not', async () => {
  const realFetch = globalThis.fetch;
  const status: Record<string, number> = {};
  globalThis.fetch = (async (input: any) => {
    const url = String(input?.url ?? input);
    const gpu = url.match(/gpu\.cbgpu\.com\/(\d)\/login/);
    return new Response('x', {status: gpu ? (status['gpu' + gpu[1]] ?? 401) : url.includes('docs.') ? 500 : 200});
  }) as typeof fetch;
  try {
    status.gpu2 = 502;
    const puts: string[] = [];
    let pending: Promise<unknown> = Promise.resolve();
    const realDate = Date, fixed = realDate.parse('2026-11-17T10:07:20.000Z');
    class FakeDate extends realDate { constructor(value?: any) { super(value === undefined ? fixed : value) } static now() { return fixed } }
    (globalThis as any).Date = FakeDate;
    try { await worker.scheduled({}, fakeEnv({failures: 0, successes: 0, incident: false}, puts), {waitUntil: (p: Promise<unknown>) => { pending = p }}); await pending } finally { (globalThis as any).Date = realDate }
    const saved = puts.map(p => p.replace(/^monitor:/, '')).map(p => { try { return JSON.parse(p) } catch { return null } }).find(Boolean);
    assert.equal(saved.checks.gpu1, true, '401 means the gateway is up');
    assert.equal(saved.checks.gpu2, false, '502 is a real failure');
    assert.equal(saved.checks.docs, false, '500 on a normal site is a failure');
    assert.equal(saved.checks.website, true);
  } finally { globalThis.fetch = realFetch }
});
