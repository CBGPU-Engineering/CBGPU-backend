# Current portal authentication and equipment workspaces

All portals now use one CBGPU email/password sign-in. There is no additional system-password or authenticator step. Developer/admin eligibility remains restricted to the configured developer identities with active approved admin accounts. Printer and laser eligibility is looked up independently from current account roles, with live session/role revalidation, CSRF checks and separate host-only session cookies/databases.

- `developers.cbgpu.com`: developer console.
- `admin.cbgpu.com`: site administration.
- `print.cbgpu.com`: printer managers; printer mappings are assigned in the admin equipment-manager settings.
- `laser.cbgpu.com`: laser-cutting managers; requires the laser-manager role.
- `manufacturing.cbgpu.com`: equipment workspace chooser.

The English sign-in markup, stylesheet and theme script are reused from the developer portal. The Next.js shells use the same `cbgpu-developer-theme` preference and generated developer stylesheet. Run `node FrontEnd/cbgpu/scripts/sync-admin-developer-style.cjs` after changing that canonical stylesheet.

Verification: `node backend/scripts/verify-developer-auth.mjs` and `node backend/scripts/verify-equipment-portals.mjs`. The latter uses an isolated gateway and temporary accounts; set `CBGPU_TEST_FRONTEND_PORT` to a preview port. For live equipment-only checks use `CBGPU_TEST_PUBLIC=1 CBGPU_TEST_FRONTEND_PORT=3000`.

The additional-password instructions in older deployment notes below are obsolete.

---

# CBGPU developer workspace

Runs as `cbgpu-developer-portal.service` (systemd user service) on `127.0.0.1:3400`.
Cloudflare Tunnel routes `https://developers.cbgpu.com` to this address. The website,
database service, and this portal are separate services.

The UI follows CBGPU's neutral surfaces, blue accent, logo, rounded controls, and
main-site dark colors. Light, dark, and system themes persist in browser storage.

## Pages

- Overview: real health probes, host utilization, anonymous visitor counts and active release.
- Table editor: searchable catalogue, server-side row search, sorting, pagination,
  column details, primary-key row editing, and current-page CSV export.
- SQL editor: one dedicated page, query templates, keyboard shortcut, CSV results.
- Deployments: **read-only** release metadata, history and build logs. There is
  no GitHub sync, build, deploy or rollback action available from this portal.
  `POST /api/deploy` deliberately returns 405, including for authenticated users.
- Logs & services: recent logs from fixed service sources, Docker status, backup
  inventory and email sender configuration status.
- Terminal: a real Bash PTY over an authenticated WebSocket. Supports ordinary
  shell commands, directory changes, Ctrl+C, Tab completion, full-screen programs,
  installed Codex, and terminal resize. Runs as the existing `cbgpu` OS user.
  Connect opens a shell; Disconnect, browser closure or logout closes it.
  Use `tmux` yourself if persistent terminal sessions are needed.


## Access and data handling

Private settings in `backend/private/developer-portal.json` contain a scrypt
password hash and random session/ingestion secrets. No password is in the browser
bundle. Sessions use Secure/HttpOnly/SameSite cookies; mutations require CSRF and
an allowed Origin. Protected results are never cached. Changing the UI theme
stores only the selected theme locally.

The developer SQL role is restricted to application tables and its own workspace
schema. It cannot read Auth passwords, Vault secrets, or act as a database
superuser. Avoid placing application secrets in public application tables.

Traffic starts when the main frontend's tracker first records a real page view;
no historical visits are invented. Daily counts use UTC and anonymous browser
identifiers, respect DNT/GPC, and retain 30 days. The chart shows the last 7 days.

The production frontend may run from an isolated release directory under
`backend/releases`. Its exact working directory and build are in the frontend's
systemd unit. For future manual releases, build the intended checkout and update
that unit; editing files in a different checkout does not change production.

## Verification

`node backend/scripts/verify-developer-workspace.cjs` exercises real local APIs,
permission boundaries, search/pagination, SQL, service logs, the interactive terminal,
read-only deployment enforcement, dark mode persistence and responsive layouts.
It creates and removes a test table only in `developer_workspace`, and removes
its temporary authenticated session. Browser screenshots are saved under `/tmp`.
The local Brave executable and frontend's puppeteer-core dependency are required.

## Browser terminal implementation

Dependencies are pinned in `package-lock.json`: node-pty, ws, xterm and addon-fit.
Vendored browser files are served locally with their upstream licenses. No
external terminal CDN or separate public shell port is required.

A CSRF-protected POST creates a single-use WebSocket ticket lasting 30 seconds.
The WebSocket also requires the authenticated cookie and allowed Origin. Session
expiry/revocation closes shells. Output is streamed rather than saved to portal
logs. There are at most four simultaneous shells, two per authenticated session.
The child receives a normal login-shell environment, without the portal's
backend API credentials being copied into process environment variables.

The systemd service runs without PrivateTmp/NoNewPrivileges restrictions so that
the shell behaves like the user's existing terminal. It has the existing user's
filesystem access and normal sudo authentication requirements.

Public HTTPS and authenticated WebSocket/PTy checks passed on 2026-10-28.
