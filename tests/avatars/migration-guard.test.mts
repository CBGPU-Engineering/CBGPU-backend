import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';

/**
 * 20261126150000_avatar_surfaces.sql renames these builders to <name>_core and puts a wrapper with the old name in
 * front of them (the wrapper adds avatarUrl/profileImageUrl). A later migration that does `create or replace` on the
 * old name would silently replace the wrapper and every user in these responses would lose its avatar. Change
 * <name>_core instead.
 */
const WRAPPED = ['message_json', 'dm_message_json', 'member_json', 'operator_nomination', 'gpu_admin_booking_item'];
const WRAPPER_MIGRATION = '20261126150000';
const dir = new URL('../../supabase/migrations/', import.meta.url);

test('no migration after the avatar wrappers redefines a wrapped builder directly', () => {
  const offenders: string[] = [];
  for (const file of fs.readdirSync(dir).filter(name => name.endsWith('.sql')).sort()) {
    if (file.slice(0, 14) <= WRAPPER_MIGRATION) continue;
    const sql = fs.readFileSync(new URL(file, dir), 'utf8').replace(/--[^\n]*/g, '');
    for (const name of WRAPPED)
      if (new RegExp(`create\\s+(or\\s+replace\\s+)?function\\s+cbgpu_schools\\.${name}\\s*\\(`, 'i').test(sql)) offenders.push(`${file}: ${name}`);
  }
  assert.deepEqual(offenders, [], `redefine cbgpu_schools.<name>_core instead of the wrapper:\n${offenders.join('\n')}`);
});

test('the guard itself recognises the pattern it protects against', () => {
  const sample = 'create or replace function cbgpu_schools.message_json(m cbgpu_schools.messages) returns jsonb as $$ select 1 $$;';
  assert.match(sample, new RegExp('create\\s+(or\\s+replace\\s+)?function\\s+cbgpu_schools\\.message_json\\s*\\(', 'i'));
  assert.ok(WRAPPED.length === 5 && fs.existsSync(new URL(`${WRAPPER_MIGRATION}_avatar_surfaces.sql`, dir)));
});
