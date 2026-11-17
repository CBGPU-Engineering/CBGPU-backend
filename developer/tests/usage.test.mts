import test from 'node:test';
import assert from 'node:assert/strict';
import {ACCOUNT_USAGE_SQL, parseAccountUsage, parseGatewayLog} from '../usage.mts';

const NOW = Date.parse('2026-11-17T12:30:00.000Z');
const line = (time: string, request: string, status: number, agent = 'node') => `172.18.0.1 - - [${time} +0000] "${request} HTTP/1.1" ${status} 123 "-" "${agent}"`;

test('counts requests per hour, last hour, services and server errors', () => {
  const log = [
    line('17/Nov/2026:12:10:00', 'GET /rest/v1/profiles?select=*&id=eq.secret', 200, 'Mozilla/5.0 (Linux; Android 14)'),
    line('17/Nov/2026:12:20:00', 'POST /auth/v1/token?grant_type=refresh_token', 200, 'Dart/3.5 (dart:io)'),
    line('17/Nov/2026:11:40:00', 'GET /storage/v1/object/x', 500),
    line('17/Nov/2026:09:05:00', 'GET /realtime/v1/websocket', 101),
    line('17/Nov/2026:09:06:00', 'POST /functions/v1/notify', 200),
    line('17/Nov/2026:09:07:00', 'GET /somewhere-else', 404),
    line('16/Nov/2026:11:00:00', 'GET /rest/v1/old', 200), // older than 24h
    'garbage line', '',
  ].join('\n');
  const usage = parseGatewayLog(log, NOW);
  assert.equal(usage.requests_24h, 6);
  assert.equal(usage.requests_1h, 3); // 12:10, 12:20 and 11:40 are within the last hour
  assert.equal(usage.client_requests_24h, 2, 'only the browser and the app count as people');
  assert.equal(usage.client_requests_1h, 2);
  assert.equal(usage.server_errors_24h, 1);
  assert.equal(usage.error_rate, 16.67);
  assert.deepEqual(usage.services, {rest: 1, auth: 1, storage: 1, realtime: 1, functions: 1, other: 1});
  assert.equal(usage.per_hour.length, 24);
  assert.equal(usage.per_hour.reduce((sum, h) => sum + h.requests, 0), 6);
  assert.equal(usage.per_hour.at(-1)!.hour_start, '2026-11-17T12:00:00.000Z');
  assert.equal(usage.per_hour.at(-1)!.requests, 2);
});

test('no traffic gives zero counts and a null error rate', () => {
  const usage = parseGatewayLog('', NOW);
  assert.equal(usage.requests_24h, 0);
  assert.equal(usage.error_rate, null);
});

test('output contains only aggregates: no paths, query strings or addresses', () => {
  const log = line('17/Nov/2026:12:10:00', 'GET /rest/v1/profiles?select=*&id=eq.secret-value', 200);
  const text = JSON.stringify(parseGatewayLog(log, NOW));
  for (const forbidden of ['profiles', 'secret-value', '172.18', 'select', 'Mozilla', 'node']) assert.ok(!text.includes(forbidden), 'leaked ' + forbidden);
});

test('account usage is validated and the SQL is read-only', () => {
  assert.deepEqual(parseAccountUsage('{"total":170,"dau":62,"wau":78,"mau":108,"new_today":1,"new_7d":4,"logins_today":120,"logins_7d":600,"logins_30d":2400}\n'), {total: 170, dau: 62, wau: 78, mau: 108, new_today: 1, new_7d: 4, logins_today: 120, logins_7d: 600, logins_30d: 2400});
  assert.equal(parseAccountUsage('{"total":"x"}'), null);
  assert.equal(parseAccountUsage('not json'), null);
  assert.ok(/^SELECT /.test(ACCOUNT_USAGE_SQL.trim()) && !/\b(INSERT|UPDATE|DELETE|DROP|ALTER|CREATE|GRANT)\b/i.test(ACCOUNT_USAGE_SQL));
});
