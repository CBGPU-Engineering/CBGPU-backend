import test from 'node:test';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import {DatabaseSync} from 'node:sqlite';
import {developerAuth} from '../auth.mts';

const USER = '11111111-1111-4111-8111-111111111111';
const SID = '22222222-2222-4222-8222-222222222222';
const EMAIL = 'admin@example.invalid';
const secret = 'test-session-secret';
const jwt = (claims: object) => ['{"alg":"none"}', JSON.stringify(claims), 'sig'].map(part => Buffer.from(part).toString('base64url')).join('.');
const hash = (value: string) => crypto.createHmac('sha256', secret).update('developer:' + value).digest('hex');
const SIDE = 3600_000;

async function setup(extra: object = {}) {
  const db = new DatabaseSync(':memory:');
  db.exec('CREATE TABLE sessions(token_hash TEXT PRIMARY KEY, csrf TEXT NOT NULL, expires INTEGER NOT NULL)');
  const settings = {sessionSecret: secret, developers: [{id: USER, email: EMAIL}], ...extra};
  const auth = developerAuth({db, settings, env: {ANON_KEY: 'anon'}, loadPrincipals: async () => [{id: USER, email: EMAIL, name: 'Admin', sessions: [{id: SID}]}]});
  await auth.init();
  const token = crypto.randomBytes(16).toString('hex');
  const insert = (expires: number, issued: number | null) => db.prepare('INSERT INTO sessions(token_hash,csrf,expires,user_id,email,auth_version,auth_session_id,auth_tokens,issued) VALUES(?,?,?,?,?,?,?,?,?)')
    .run(hash(token), 'csrf', expires, USER, EMAIL, hash('account-only-v1:' + USER), SID, null, issued);
  const req = {headers: {cookie: '__Host-cbgpu-developer=' + token}};
  return {db, auth, token, insert, req};
}
const fakeRes = () => {const headers: string[] = []; return {headers, appendHeader: (_name: string, value: string) => { headers.push(value) }}};

test('active sessions slide forward, at most hourly, and the cookie lifetime follows', async () => {
  const {auth, insert, req, db} = await setup();
  const now = Date.now();
  insert(now + 2 * SIDE, now - 6 * SIDE);
  let s = auth.session(req);
  assert.ok(s, 'session is valid');
  const res = fakeRes();
  assert.equal(auth.renew(req, res, s), true);
  const row: any = db.prepare('SELECT expires FROM sessions').get();
  assert.ok(row.expires >= now + 23 * SIDE && row.expires <= now + 24 * SIDE + 5000, 'idle window is 24h');
  assert.match(res.headers[0], /Max-Age=(8\d{4})\b/);
  assert.match(res.headers[0], /HttpOnly; Secure; SameSite=Strict/);
  s = auth.session(req);
  assert.equal(auth.renew(req, fakeRes(), s), false, 'no second write within the hour');
});

test('renewal never exceeds the absolute maximum age', async () => {
  const {auth, insert, req, db} = await setup({sessionIdleHours: 24, sessionMaxDays: 2});
  const now = Date.now();
  insert(now + 2 * SIDE, now - 40 * SIDE); // issued 40h ago, max 48h
  const s = auth.session(req);
  assert.equal(auth.renew(req, fakeRes(), s), true);
  const row: any = db.prepare('SELECT expires, issued FROM sessions').get();
  assert.ok(row.expires <= row.issued + 48 * SIDE + 1000, 'capped at issued + 2 days');
  assert.ok(row.expires >= row.issued + 48 * SIDE - 1000, 'extended up to the cap, not beyond');
});

test('browser-refreshed tokens are verified and stored; mismatching sessions are refused', async () => {
  const {auth, insert, req} = await setup();
  const now = Date.now();
  insert(now + 5 * SIDE, now);
  const s = auth.session(req);
  const realFetch = globalThis.fetch;
  globalThis.fetch = (async () => new Response(JSON.stringify({id: USER, aud: 'authenticated', role: 'authenticated', email: EMAIL}), {status: 200, headers: {'content-type': 'application/json'}})) as typeof fetch;
  try {
    const good = {access_token: jwt({sub: USER, session_id: SID, exp: Math.floor(now / 1000) + 3600}), refresh_token: 'refresh-2'};
    assert.deepEqual(await auth.rememberTokens(s, good), {ok: true});
    assert.deepEqual(auth.nativeSession(auth.session(req)), good, 'the restored session now serves the fresh tokens');
    await assert.rejects(auth.rememberTokens(s, {...good, access_token: jwt({sub: USER, session_id: '33333333-3333-4333-8333-333333333333', exp: Math.floor(now / 1000) + 3600})}), {status: 409});
    await assert.rejects(auth.rememberTokens(s, {...good, access_token: jwt({sub: '44444444-4444-4444-8444-444444444444', session_id: SID, exp: Math.floor(now / 1000) + 3600})}), {status: 409});
    await assert.rejects(auth.rememberTokens(s, {access_token: jwt({sub: USER, session_id: SID, exp: 1}), refresh_token: 'x'}), {status: 409});
    await assert.rejects(auth.rememberTokens(s, {access_token: 'x', refresh_token: 1 as any}), {status: 400});
  } finally { globalThis.fetch = realFetch; auth.close() }
});
