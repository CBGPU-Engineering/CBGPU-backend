import assert from 'node:assert/strict';
import test from 'node:test';
import { checkedContainer, parseTerminalCommand, redactTerminal, terminalArguments } from './project-terminal.mts';

const id = '0'.repeat(64);
const namespace = 'cbgpu-compose-11111111-1111-4111-8111-111111111111';

function state({ project = namespace, service = 'hermes', privileged = false, network = 'private' } = {}) {
  return [id, 'true', '"root"', JSON.stringify(project), JSON.stringify(service), String(privileged),
    '["no-new-privileges:true"]', '""', '"private"', JSON.stringify(network), '""'].join('\t');
}

test('targets only the exact isolated container ID, passing remaining arguments literally', () => {
  const input = parseTerminalCommand({ service: 'hermes', argv: ['/usr/bin/printf', '%s', 'a; b'], cwd: '/opt/data/workspace' });
  const container = checkedContainer(state(), namespace, input.service);
  assert.deepEqual(terminalArguments(container, input), [
    'exec', '--user', '10000:10000', '--workdir', '/opt/data/workspace', id,
    '/usr/bin/timeout', '-k', '1', '--', '10', '/usr/bin/printf', '%s', 'a; b',
  ]);
  for (const row of [state({ project: 'other' }), state({ service: 'db' }), state({ privileged: true }), state({ network: 'host' })]) {
    assert.throws(() => checkedContainer(row, namespace, input.service));
  }
});

test('rejects missing executable, Docker-style options, path-like services and root identities', () => {
  const valid = { service: 'hermes', argv: ['pwd'] };
  for (const bad of [
    { argv: [] }, { argv: ['--privileged'] }, { argv: ['echo', 'bad\0value'] },
    { service: '../hermes' }, { cwd: '../host' }, { uid: 0 }, { uid: 22, gid: 0 }, { gid: 22 },
  ]) assert.throws(() => parseTerminalCommand({ ...valid, ...bad }), { status: 400 });
  assert.equal(terminalArguments({ id, user: '22:23' }, parseTerminalCommand(valid))[2], '22:23');
});

test('redacts known deployment secrets and supported credential formats in output', () => {
  const secret = 'fixture-password';
  assert.equal(redactTerminal(`status ${secret} cbgpu_ai_fixture-token`, { SOME_PASSWORD: secret }), 'status [REDACTED] [REDACTED]');
});
