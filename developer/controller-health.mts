/** Server-side compatibility selection, never a request-supplied unit or command. */
export function controllerHealthCommand(mode = process.env.CBGPU_CONTROLLER_HEALTH_MODE || 'native'): string[] {
  if (mode === 'native') return ['--user', 'is-active', 'cbgpu-gpu-controller.service'];
  if (mode === 'legacy') return ['is-active', 'cbgpu-daemon.service'];
  throw new Error('Unsupported controller health mode');
}
export async function controllerHealth(run: (file: string, args: string[], options: {timeout: number}) => Promise<{stdout: string}>, mode?: string): Promise<boolean> {
  try {
    const result = await run('systemctl', controllerHealthCommand(mode), {timeout: 3000});
    return result.stdout.trim() === 'active';
  } catch { return false; }
}
