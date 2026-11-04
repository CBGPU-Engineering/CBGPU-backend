"""Root-only code rollout; do not restart the user's GPU containers."""
import os,subprocess,time
marker='/run/cbgpu-daemon-preserve-idle-once'
fd=os.open(marker,os.O_WRONLY|os.O_CREAT|os.O_TRUNC,0o600);os.close(fd)
try:
 subprocess.run(['systemctl','restart','cbgpu-daemon.service'],check=True)
 time.sleep(35)
 subprocess.run(['systemctl','is-active','--quiet','cbgpu-daemon.service'],check=True)
finally:
 os.unlink(marker)
