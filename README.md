# CBGPU backend

This directory runs CBGPU's Supabase-compatible backend on this computer. It includes PostgreSQL 17, Auth, REST/RPC APIs, Realtime, Storage, Edge Functions, Studio, the connection pooler, email configuration, and the existing scheduled tasks.

The public API is `https://cbgpu.com/backend`, routed through the Next.js server and the existing Cloudflare Tunnel. Only the Auth, REST, Storage, Realtime, and Functions API paths are exposed. Studio and database access remain local. The frontend, Android app, GPU controller, SSH gateways, and materials service must use the same backend and matching API keys.

Email delivery still uses Resend and device push delivery still uses Firebase, as before. Their server credentials are stored in ignored files, never in browser code or Git. Existing user UUIDs and password hashes are preserved. Users must sign in again after the backend switch because the JWT signing keys change. Android v1.0.7 (build 8) uses this backend and adds the official EduSet login bridge.

## Operations

```bash
cd /home/cbgpu/CBGPU-live/backend
docker compose up -d --wait
docker compose ps
python3 scripts/verify-backend.py
node scripts/verify-realtime.cjs
python3 scripts/verify-push-credentials.py
scripts/backup.sh
```

Docker services restart automatically unless explicitly stopped. A user systemd timer runs daily backups at 03:30 UTC. Backups contain the database, roles, uploaded files, encryption keys, and runtime configuration. Keep a separate protected off-machine copy for disaster recovery.

Do not use `docker compose down -v` or `reset.sh` on the production backend. They can remove persistent data.

## Private state

`.env`, `functions.env`, `private/`, `backups/`, database data, and uploaded files are excluded from Git. Never commit their contents. The `.env.example` file is only a template; its sample credentials must never be used for a live deployment. Docker configuration is pinned to Supabase self-hosted release `self-hosted/v0.8.3`.

Studio is available locally on port 54324. The private API gateway listens on `127.0.0.1:54321`. No PostgreSQL port is published to the network.

## Final cutover

`scripts/host-cutover.sh` is a narrow root helper for the existing root-owned GPU controller. It preserves idle containers, pauses the controller, and waits for the final data synchronization. The assistant enables a reversible write guard on the old backend, refreshes accounts/application data/files with `scripts/final-sync.py`, verifies the results, and runs `scripts/activate-local-backend.py`. The helper then starts the GPU controller against the local backend. If activation fails or times out, the helper restores the previous controller configuration.

The old hosted project is retained for rollback. Its scheduled jobs and application writes must remain disabled after a successful switch to avoid divergent data and duplicate notifications. Do not delete the hosted project until the new deployment has been reviewed in normal use.

## Verification performed

- Existing account validation, authenticated profile access, anonymous access restrictions, and password-reset URL generation.
- Matching application table, RLS-policy, foreign-key, and storage-bucket structure.
- All uploaded files copied and checked by SHA-256; ownership metadata preserved.
- Realtime WebSocket connection and channel join, including through the frontend reverse proxy.
- Edge Function access and rejection of unauthorized account deletion.
- SMTP TLS/authentication and Firebase service-account authentication without sending test messages to real users.
- An isolated database restore with account/table/file metadata and decryptable Vault secrets.

Actual message delivery, physical device behavior, and ongoing production workload should be monitored after cutover. No synthetic notifications or emails were sent to users during these checks.
