import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const docker = '/usr/bin/docker';
const serviceName = /^[A-Za-z0-9][A-Za-z0-9_-]{0,60}$/;
const executable = /^[A-Za-z0-9_./][A-Za-z0-9_./+-]{0,255}$/;
const active = new Set<string>();
const outputLimit = 32 * 1024;

function problem(message: string, status: number): never {
  throw Object.assign(new Error(message), { status });
}

export function parseTerminalCommand(input: unknown) {
  const value = input as { service?: unknown; argv?: unknown; cwd?: unknown; uid?: unknown; gid?: unknown };
  if (!value || typeof value !== 'object' || Array.isArray(value) ||
      typeof value.service !== 'string' || !serviceName.test(value.service) ||
      !Array.isArray(value.argv) || value.argv.length < 1 || value.argv.length > 32 ||
      value.argv.some(part => typeof part !== 'string' || part.length > 2048 || part.includes('\0')) ||
      !executable.test(value.argv[0]) ||
      value.cwd !== undefined && (typeof value.cwd !== 'string' || !value.cwd.startsWith('/') || value.cwd.length > 512 || /[\0\r\n]/.test(value.cwd)) ||
      [value.uid, value.gid].some(id => id !== undefined && (typeof id !== 'number' || !Number.isInteger(id) || id < 1 || id > 2147483647)) ||
      value.gid !== undefined && value.uid === undefined) {
    return problem('Check the service, command arguments, working directory and non-root user.', 400);
  }
  return { service: value.service, argv: value.argv as string[], cwd: value.cwd as string | undefined,
    uid: value.uid as number | undefined, gid: value.gid as number | undefined };
}

const inspectFormat = [
  '{{.Id}}', '{{.State.Running}}', '{{json .Config.User}}',
  '{{json (index .Config.Labels "com.docker.compose.project")}}',
  '{{json (index .Config.Labels "com.docker.compose.service")}}',
  '{{.HostConfig.Privileged}}', '{{json .HostConfig.SecurityOpt}}',
  '{{json .HostConfig.PidMode}}', '{{json .HostConfig.IpcMode}}',
  '{{json .HostConfig.NetworkMode}}', '{{json .HostConfig.UTSMode}}',
].join('\t');

export function checkedContainer(line: string, namespace: string, service: string) {
  const [id, running, userJson, projectJson, serviceJson, privileged, securityJson, ...modes] = line.trim().split('\t');
  if (!/^[a-f0-9]{64}$/.test(id) || running !== 'true' || privileged !== 'false' ||
      JSON.parse(projectJson) !== namespace || JSON.parse(serviceJson) !== service ||
      !JSON.parse(securityJson)?.some((option: string) => /^no-new-privileges[=:]true$/.test(option)) ||
      modes.length !== 4 || modes.some(mode => /^(host\b|container:)/.test(JSON.parse(mode)))) {
    return problem('This service is not an isolated running project container.', 409);
  }
  return { id, user: JSON.parse(userJson) as string };
}

export function terminalArguments(container: { id: string; user: string }, input: ReturnType<typeof parseTerminalCommand>) {
  const configured = /^[1-9]\d{0,9}(?::[1-9]\d{0,9})?$/.test(container.user) ? container.user.split(':') : [];
  const uid = input.uid ?? Number(configured[0] || 10000);
  const gid = input.gid ?? Number(input.uid !== undefined ? uid : configured[1] || uid);
  return ['exec', '--user', `${uid}:${gid}`, ...(input.cwd ? ['--workdir', input.cwd] : []),
    container.id, '/usr/bin/timeout', '-k', '1', '--', '10', ...input.argv];
}

export function redactTerminal(text: string, environment: Record<string, unknown>) {
  const secrets = Object.entries(environment)
    .filter(([key, value]) => !key.startsWith('CBGPU_') || key === 'CBGPU_API_KEY' || /TOKEN|SECRET|KEY|PASSWORD/i.test(key))
    .map(([, value]) => value).filter((value): value is string => typeof value === 'string' && value.length >= 4)
    .sort((a, b) => b.length - a.length);
  for (const secret of new Set(secrets)) text = text.replaceAll(secret, '[REDACTED]');
  return text.replace(/\b(?:cbgpu_ai_[\w-]+|eyJ[\w-]+\.eyJ[\w-]+\.[\w-]+|\d{6,}:[A-Za-z0-9_-]{25,})\b/g, '[REDACTED]');
}

export async function runProjectTerminal(project: string, services: string[], input: ReturnType<typeof parseTerminalCommand>, environment: Record<string, unknown>) {
  if (!services.includes(input.service)) return problem('Service is not in the current deployment.', 404);
  if (active.has(project) || active.size >= 8) return problem('Another terminal command is running. Try again shortly.', 429);
  active.add(project);
  try {
    const namespace = 'cbgpu-compose-' + project;
    let container: ReturnType<typeof checkedContainer>;
    try {
      const { stdout } = await exec(docker, ['inspect', '--type=container', '--format', inspectFormat,
        `${namespace}-${input.service}`], { timeout: 2000, maxBuffer: 4096 });
      container = checkedContainer(stdout, namespace, input.service);
    } catch {
      return problem('This service is not an isolated running project container.', 409);
    }
    const result = (stdout: string, stderr: string, exitCode: number | null, truncated = false) => ({
      service: input.service, exit_code: exitCode, timed_out: exitCode === 124 || exitCode === 137,
      truncated, stdout: redactTerminal(stdout, environment), stderr: redactTerminal(stderr, environment),
    });
    try {
      const { stdout, stderr } = await exec(docker, terminalArguments(container, input), { timeout: 13500, maxBuffer: outputLimit });
      return result(stdout, stderr, 0);
    } catch (error) {
      const failure = error as { code?: unknown; killed?: boolean; stdout?: string; stderr?: string };
      if (failure.code === 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER') return result(failure.stdout || '', failure.stderr || '', null, true);
      if (failure.killed) return { ...result('', '', null), timed_out: true };
      if (typeof failure.code !== 'number' || failure.code === 125) return problem('The Docker terminal is temporarily unavailable.', 503);
      if (/exec.*\/usr\/bin\/timeout.*(not found|no such file)/i.test(failure.stderr || '')) {
        return problem('This image needs /usr/bin/timeout to support terminal commands.', 409);
      }
      return result(failure.stdout || '', failure.stderr || '', failure.code);
    }
  } finally {
    active.delete(project);
  }
}
