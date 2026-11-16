#!/usr/bin/env node
// Operator-only alert: reads the aggregate result written by check.mts and sends an app push to the admins
// when the school connection is expiring/expired or the probe is unhealthy/overdue. It never reads tokens,
// student records or user sessions; it only sends a short text through the internal notify function.
//   node alert.mts            run once (sends and records state)
//   node alert.mts --dry-run  print the decision only (no push, no state change)
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {parseEnv} from 'node:util';
import {pathToFileURL} from 'node:url';

const HOUR = 3600_000;
const OVERDUE_MS = 15 * 60_000; // check.mts runs every 5 minutes
const REPEAT_MS = {critical: 3 * HOUR, warning: 12 * HOUR};
const LINK = 'https://admin.cbgpu.com/admin/study?from=app';

/** Pure decision: what (if anything) to tell the admins. */
export function decide(status, now, prev: {key?:string;sentAt?:number} = {}) {
  const checkedAt = status?.checked_at ? Date.parse(status.checked_at) : NaN;
  const expiresAt = status?.connection_expires_at ? Date.parse(status.connection_expires_at) : NaN;
  const hoursLeft = Number.isFinite(expiresAt) ? (expiresAt - now) / HOUR : null;
  let key = 'ok', level = null, title = '', body = '';
  if (!status || !Number.isFinite(checkedAt) || now - checkedAt > OVERDUE_MS || checkedAt - now > 5 * 60_000) {
    key = 'probe_stalled'; level = 'critical';
    title = '자습 상태 점검이 멈췄어요'; body = '자습 연결 점검이 15분 넘게 갱신되지 않았습니다. 서버의 점검 타이머를 확인해 주세요.';
  } else if (status.detail === 'shared_connection_missing') {
    key = 'connection_missing'; level = 'critical';
    title = '자습 토큰이 등록되지 않았어요'; body = '학교 자습 연결 토큰이 없습니다. 관리자 자습 설정에서 토큰을 등록해 주세요.';
  } else if (status.detail === 'shared_connection_expired' || (hoursLeft !== null && hoursLeft <= 0)) {
    key = 'connection_expired'; level = 'critical';
    title = '자습 토큰이 만료됐어요'; body = '학교 자습 연결 토큰이 만료되어 자습 조회가 실패합니다. 지금 자습 설정에서 새 토큰을 등록해 주세요.';
  } else if (status.status === 'unavailable') {
    key = 'school_unavailable'; level = 'critical';
    title = '자습 조회가 실패하고 있어요'; body = `학교 자습 시스템 조회에 실패했습니다(${String(status.detail || 'unknown').slice(0, 40)}). 확인이 필요합니다.`;
  } else if (status.status === 'degraded' && (status.failed_rooms || 0) > 0) {
    key = 'rooms_degraded'; level = 'warning';
    title = '일부 자습실 조회가 실패해요'; body = `${status.failed_rooms}개 자습실 구성을 불러오지 못했습니다.`;
  } else if (hoursLeft !== null && hoursLeft <= 24) {
    key = 'expiring_24h'; level = 'warning';
    title = '자습 토큰이 하루 안에 만료돼요'; body = `학교 자습 연결 토큰이 약 ${Math.max(1, Math.round(hoursLeft))}시간 뒤 만료됩니다. 미리 새 토큰을 등록해 주세요.`;
  }
  const lastKey = prev.key || 'ok';
  if (key === 'ok') {
    return lastKey === 'ok'
      ? {send: false, key}
      : {send: true, key, level: 'info', title: '자습 연결이 정상으로 돌아왔어요', body: '자습 조회가 다시 정상 동작합니다.', recovered: true};
  }
  const changed = key !== lastKey;
  const due = !prev.sentAt || now - prev.sentAt >= REPEAT_MS[level];
  return {send: changed || due, key, level, title, body};
}

async function loadSettings() {
  const unit = await fs.readFile(path.join(os.homedir(), '.config/systemd/user/cbgpu-frontend.service'), 'utf8');
  const root = unit.match(/^WorkingDirectory=(.+)$/m)?.[1];
  if (!root) throw Error('active_release');
  const web = parseEnv(await fs.readFile(path.join(root, '.env.production.local'), 'utf8'));
  const fnEnvPath = process.env.CBGPU_FUNCTIONS_ENV || path.join(os.homedir(), 'CBGPU-live/backend/functions.env');
  const fn = parseEnv(await fs.readFile(fnEnvPath, 'utf8'));
  if (!fn.INTERNAL_TRIGGER_SECRET) throw Error('internal_secret_missing');
  return {anon: web.NEXT_PUBLIC_SUPABASE_ANON_KEY, secret: fn.INTERNAL_TRIGGER_SECRET};
}

async function main() {
  const dry = process.argv.includes('--dry-run');
  const folder = path.join(os.homedir(), '.local/share/cbgpu-study-monitor');
  const statusFile = path.join(folder, 'status.json'), stateFile = path.join(folder, 'alert-state.json');
  let status: Record<string,any> | null = null, prev: {key?:string;sentAt?:number} = {};
  try { status = JSON.parse(await fs.readFile(statusFile, 'utf8')); } catch {}
  try { prev = JSON.parse(await fs.readFile(stateFile, 'utf8')); } catch {}
  const d = decide(status, Date.now(), prev);
  if (dry || !d.send) { console.log(JSON.stringify({dry, ...d, body: undefined})); return; }
  const {anon, secret} = await loadSettings();
  const res = await fetch('http://127.0.0.1:54321/functions/v1/notify-admin-event', {
    method: 'POST',
    headers: {'Content-Type': 'application/json', apikey: anon, Authorization: `Bearer ${secret}`},
    body: JSON.stringify({type: 'ops_alert', title: d.title, body: d.body, url: LINK}),
    signal: AbortSignal.timeout(15_000),
  });
  const out = await res.json().catch(() => ({}));
  if (!res.ok) throw Error(`notify_http_${res.status}`);
  await fs.mkdir(folder, {recursive: true, mode: 0o700});
  const next = {key: d.key, level: d.level, sentAt: Date.now(), notified: out.notified ?? null};
  const temp = stateFile + '.tmp-' + process.pid;
  await fs.writeFile(temp, JSON.stringify(next) + '\n', {mode: 0o600});
  await fs.rename(temp, stateFile);
  console.log(JSON.stringify({sent: true, key: d.key, notified: out.notified ?? null}));
}

if (import.meta.url === pathToFileURL(process.argv[1] || '').href) {
  main().catch(e => { console.error('alert_failed:', /^[a-z_0-9]+$/.test(e.message) ? e.message : 'error'); process.exit(1); });
}
