# Production delivery

The production host pulls only the current `main` commit of **CBGPU-web** and
**CBGPU-backend**, after the repository's **CI** workflow completed successfully
for a **push** event on that exact commit. Pull requests never run on this host.
No public webhook, SSH deployment key in GitHub Actions, or self-hosted PR runner
is needed. Repository settings should prevent force pushes and branch deletion.

## Release process

1. A systemd timer checks once per minute after the preceding run finishes.
2. The agent verifies the allowed repository, branch, commit, event and CI result.
3. Source is fetched into a detached, commit-specific Git worktree. Production
   secrets come from a private runtime env file, never Git or build artifacts.
4. The web release gets its own lockfile installation and production build. A
   separate loopback process checks the health endpoint and all public workspace
   hostnames. The existing APK is retained outside the source tree.
5. The service switches to the verified build. Post-switch health must report the
   exact commit. On failure, previous unit files are restored and services checked.
   A brief restart is possible; this is not a zero-downtime proxy rollout.
6. Backend app changes validate syntax/CI and mail-queue tests, preview recruitment
   with an isolated empty database and no email credentials, then snapshot runtime
   SQLite databases. Portal/deployment gateway/recruitment services restart only
   for changed component paths. Active user deployments defer a backend restart.
7. GitHub production Deployments, the existing developers-page deployment history,
   private redacted logs, and retryable email notifications record the result.

The host agent is separately installed and does not replace itself from incoming
source. Deployment-policy changes require an explicit host installation/review.
A crash journal restores previous service units after an interrupted cutover.
A failed SHA is not retried indefinitely; an operator must investigate and retry.

## Boundaries

Database/schema migrations, database engine images, storage volumes, GPU notebook
startup/controller code, the AGPL LaTeX component and system changes are **not**
blindly applied on push. A backend commit changing protected paths waits for a
separate operational rollout. This avoids applying destructive changes merely
because unit tests passed. Materials/LaTeX/controller repositories retain their
own CI and operational rollout requirements. Android uses its signed tag-release
workflow; pushing application code does not silently install an APK on devices.

Runtime data remains in the existing backend `private/`, storage and DB volumes.
The root GPU controller and running notebook jobs are not stopped by delivery.
Code rollback does not roll databases backward; predeploy snapshots are for an
explicit recovery, not an automatic overwrite of new user data.

## Host operations

Installed agent: `~/.local/lib/cbgpu-delivery/agent.py`.
State/config/logs/releases: `~/.local/share/cbgpu-delivery/` (private).

```bash
python3 ~/.local/lib/cbgpu-delivery/agent.py --status
journalctl --user -u cbgpu-delivery.service -n 50
systemctl --user list-timers cbgpu-delivery.timer
# Stop automatic polls while investigating; a running build finishes first.
systemctl --user stop cbgpu-delivery.timer
python3 ~/.local/lib/cbgpu-delivery/agent.py --pause
# Re-check a corrected/previously failed commit.
python3 ~/.local/lib/cbgpu-delivery/agent.py --resume
python3 ~/.local/lib/cbgpu-delivery/agent.py --retry web
# Restore the previous service configuration and block that bad commit.
python3 ~/.local/lib/cbgpu-delivery/agent.py --rollback web
systemctl --user start cbgpu-delivery.timer
```

Do not edit a release directory. Commit application changes to the component
repository. Changes to an older monorepo folder are not deployment inputs.
The agent uses the host's authenticated GitHub CLI; if credentials expire, it
fails closed and leaves the existing service running.

## Backups and recovery

Daily backend backup includes a consistent recruitment SQLite snapshot and its
form key, developer state and current delivery configuration/history. These are
included in the existing encrypted offsite backup. Backup configuration points
to the existing runtime backend so source extraction does not relocate data.
Restore verification uses a separate, network-isolated PostgreSQL container;
it never restores over the production database. Source commits remain in GitHub;
rebuild the pinned release and restore private runtime files to recover the host.

## Checks

```bash
python3 -m unittest discover -s operations/delivery -p 'test_*.py'
```

Tests cover exact-main CI authorization, rejected repositories, failed-health
rollback, protected schema changes, user-build deferral and divergent history.
Source licensing follows this backend's Apache-2.0 license.

Generated release caches retain at least the five newest versions per component
and all active/rollback unit targets. Older unused versions become eligible for
cleanup after seven days. New builds defer below 15 GiB free disk space. Code
archives inside encrypted backups are independent of this cache retention.

Three consecutive GitHub connection/authentication failures queue an operator
alert; a subsequent verified CI check queues a recovery alert. Monitoring and
code delivery are independent: an unavailable GitHub API never stops the running
application. A manually rolled-back commit remains blocked until explicitly retried.

After independently applying and verifying a protected infrastructure migration,
an operator can acknowledge that exact CI-passed backend main commit with
`agent.py --approve-backend FULL_SHA --only backend`. This does not execute any
migration: it records the operator's completed rollout and permits application
activation for that one commit. Future protected commits still stop for review.

Public Next.js static assets from three previous builds are retained during delivery so an already-open admin or user tab can finish loading after a release. Original file lists keep this retention bounded; current assets are never overwritten, and server files or symlinks are never copied.

## Edge functions

The edge-functions container serves `<runtime_backend>/volumes/functions`, which a release checkout never replaces.
When a release changes files under `volumes/functions/` (an approved infrastructure rollout), `deploy_backend` copies
only those files into the runtime folder (never deleting), keeps the previous copies in
`<runtime_backend>/backups/functions-<sha>/`, restarts `cbgpu-supabase-edge-functions`
(`CBGPU_FUNCTIONS_CONTAINER` to override) and requires it to be healthy; otherwise it restores the previous files and
restarts again. A `functions_synced` event lists the copied files.
