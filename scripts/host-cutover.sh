#!/usr/bin/env bash
# Coordinate the root GPU controller with the assistant's final database sync.
set -euo pipefail
if (( EUID != 0 )); then
  echo "Run: sudo $0"
  exit 1
fi
backend_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
state_dir="$backend_dir/private/cutover"
mkdir -p "$state_dir"
chmod 700 "$state_dir"
chown cbgpu:cbgpu "$state_dir"
cp -p /home/cbgpu/CBGPU/Daemon/config.yaml "$state_dir/controller-config.before"
cp -p /home/cbgpu/.config/cbgpu/service_role_key "$state_dir/controller-key.before"
rm -f "$state_dir/controller-paused" "$state_dir/backend-active" "$state_dir/abort" "$state_dir/controller-active"
completed=false
rollback() {
  if [[ "$completed" != true ]]; then
    cp -p "$state_dir/controller-config.before" /home/cbgpu/CBGPU/Daemon/config.yaml
    cp -p "$state_dir/controller-key.before" /home/cbgpu/.config/cbgpu/service_role_key
    touch /run/cbgpu-daemon-preserve-idle-once
    systemctl start cbgpu-daemon.service
    echo 'Controller restored to its previous backend. Tell the assistant cutover did not finish.'
  fi
}
trap rollback EXIT
trap 'exit 1' INT TERM
touch /run/cbgpu-daemon-preserve-idle-once
systemctl stop cbgpu-daemon.service
touch "$state_dir/controller-paused"
chmod 644 "$state_dir/controller-paused"
echo 'GPU controller paused safely. Tell the assistant this command is running; keep this terminal open.'
for ((attempt=0; attempt<900; attempt++)); do
  if [[ -f "$state_dir/abort" ]]; then exit 1; fi
  if [[ -f "$state_dir/backend-active" ]]; then
    systemctl start cbgpu-daemon.service
    systemctl is-active --quiet cbgpu-daemon.service
    touch "$state_dir/controller-active"
    chmod 644 "$state_dir/controller-active"
    completed=true
    sleep 10
    rm -f /run/cbgpu-daemon-preserve-idle-once
    echo 'GPU controller is running against the local backend.'
    exit 0
  fi
  sleep 1
done
echo 'Timed out waiting for final backend activation.'
exit 1
