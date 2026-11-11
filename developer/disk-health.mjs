import fs from 'node:fs';

// Count each mounted filesystem once, even if the data mount is unavailable.
export function diskHealth(paths = ['/home/cbgpu', '/srv/cbgpu-data'], io = fs) {
  const seen = new Set(), disks = [];
  for (const path of paths) {
    try {
      const device = io.statSync(path).dev;
      if (seen.has(device)) continue;
      const stat = io.statfsSync(path);
      if (!(stat.blocks > 0 && stat.bsize > 0)) continue;
      seen.add(device);
      const total_bytes = stat.blocks * stat.bsize;
      const used_bytes = (stat.blocks - stat.bfree) * stat.bsize;
      const available_bytes = stat.bavail * stat.bsize;
      disks.push({path, total_bytes, used_bytes, available_bytes, percent: 100 * used_bytes / total_bytes});
    } catch { /* Missing drives do not become fictional free capacity. */ }
  }
  const sum = key => disks.reduce((n, disk) => n + disk[key], 0);
  const total_bytes = sum('total_bytes'), used_bytes = sum('used_bytes');
  return {total_bytes, used_bytes, available_bytes: sum('available_bytes'),
    percent: total_bytes ? 100 * used_bytes / total_bytes : null,
    // A full system disk still needs attention even with space on the data SSD.
    healthy: disks.length > 0 && disks.every(d => 100 * (1 - d.available_bytes / d.total_bytes) < 95), disks};
}
