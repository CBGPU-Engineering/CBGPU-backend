import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { availableGiB, composeStoragePool, validateManagedVolume } from './compose-storage.mts';

const project = {
  id: '00000000-0000-4000-8000-000000000111',
  git_url: 'https://github.com/example/agent',
  git_branch: 'main',
  env_vars: { CBGPU_ROOT_DIRECTORY: 'compose', CBGPU_COMPOSE_FILE: 'compose.yaml' },
};
const namespace = 'cbgpu-compose-' + project.id;
const name = namespace + '-storage';

test('unregistered projects keep the default pool and cannot request a host path', async t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'cbgpu-storage-test-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const base = path.join(root, 'compose-deployments');
  fs.mkdirSync(base);
  const values = { ...project, env_vars: { ...project.env_vars, CBGPU_COMPOSE_SECONDARY_POOL: '/tmp' } };
  assert.deepEqual(await composeStoragePool(values, base, path.join(root, 'secondary')), { id: 'default', directory: base });
  assert.ok(availableGiB(base) >= 0);
  const registry = path.join(root, 'compose-volume-pools');
  fs.mkdirSync(registry);
  fs.writeFileSync(path.join(registry, project.id + '.json'), '{}', { mode: 0o600 });
  await assert.rejects(composeStoragePool(values, base, path.join(root, 'missing-mount')));
  const pool = path.join(root, 'secondary');
  fs.mkdirSync(path.join(pool, project.id, 'data'), { recursive: true });
  await assert.rejects(composeStoragePool(values, base, pool), /disk is unavailable/);
});

test('managed volume must match the registered project, mount source and local driver', () => {
  const directory = '/storage/agent/data';
  const volume = {
    Name: name,
    Driver: 'local',
    Labels: { 'com.docker.compose.project': namespace, 'com.docker.compose.volume': 'storage' },
    Options: { device: directory, type: 'none', o: 'bind' },
  };
  assert.equal(validateManagedVolume(volume, name, 'storage', directory, namespace), true);
  assert.equal(validateManagedVolume(volume, name, 'storage', '/tmp', namespace), false);
  assert.equal(validateManagedVolume(volume, name, 'storage', directory, 'other-project'), false);
  assert.equal(validateManagedVolume({ ...volume, Options: { ...volume.Options, o: 'ro,bind' } }, name, 'storage', directory, namespace), false);
});
