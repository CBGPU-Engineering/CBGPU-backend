import fs from 'node:fs';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const run = promisify(execFile);
const projectId = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
const volumeKey = /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/;
const gib = 1024 ** 3;

export type ComposeStorageProject = {
  id: string;
  git_url: string;
  git_branch: string;
  env_vars?: Record<string, string>;
};

export function availableGiB(directory: string) {
  const disk = fs.statfsSync(directory);
  return Math.floor(disk.bavail * disk.bsize / gib);
}

export function validateManagedVolume(volume: any, name: string, key: string, directory: string, namespace: string) {
  return volume?.Name === name && volume.Driver === 'local' &&
    volume.Labels?.['com.docker.compose.project'] === namespace &&
    volume.Labels?.['com.docker.compose.volume'] === key &&
    volume.Options?.device === directory &&
    volume.Options?.type === 'none' && volume.Options?.o === 'bind';
}

// The optional registry is provisioned by the operator. Project settings never supply a host path.
export async function composeStoragePool(project: ComposeStorageProject, base: string, configuredPool = process.env.CBGPU_COMPOSE_SECONDARY_POOL) {
  if (!configuredPool) return { id: 'default', directory: base };
  if (!path.isAbsolute(configuredPool) || !projectId.test(project.id)) throw Error('Invalid managed storage configuration');
  const registration = path.join(path.dirname(base), 'compose-volume-pools', project.id + '.json');
  if (!fs.existsSync(registration)) return { id: 'default', directory: base };

  const pool = fs.realpathSync(configuredPool);
  const projectDirectory = path.join(pool, project.id);
  const dataDirectory = path.join(projectDirectory, 'data');
  const device = fs.statSync(pool).dev;
  if (fs.realpathSync(projectDirectory) !== projectDirectory || fs.realpathSync(dataDirectory) !== dataDirectory ||
      fs.statSync(dataDirectory).dev !== device || device === fs.statSync(base).dev) {
    throw Error('Managed storage disk is unavailable');
  }
  const manifest = fs.lstatSync(registration);
  if (!manifest.isFile() || manifest.size > 4096 || manifest.uid !== process.getuid?.() || (manifest.mode & 0o077)) {
    throw Error('Invalid managed volume registration');
  }
  const config = JSON.parse(fs.readFileSync(registration, 'utf8'));
  if (config.git_url !== project.git_url || config.git_branch !== project.git_branch ||
      config.root !== project.env_vars?.CBGPU_ROOT_DIRECTORY || config.compose !== project.env_vars?.CBGPU_COMPOSE_FILE ||
      typeof config.volume !== 'string' || !volumeKey.test(config.volume)) {
    throw Error('Managed volume requires its registered Compose source');
  }
  const namespace = 'cbgpu-compose-' + project.id;
  const name = namespace + '-' + config.volume;
  const { stdout } = await run('docker', ['volume', 'inspect', name], { timeout: 5000, maxBuffer: 8192 });
  const [volume] = JSON.parse(stdout);
  if (!validateManagedVolume(volume, name, config.volume, dataDirectory, namespace)) throw Error('Managed volume is unavailable');
  return { id: 'secondary', directory: pool };
}
