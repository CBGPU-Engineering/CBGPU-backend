# Shared school connection renewal

A local systemd user timer checks the shared school connection every 15 minutes.
Connections with more than 24 hours remaining are left alone. When renewal is
due, the job signs in through the official EduSet login page, accepts only the
site's normal verification, verifies the returned token against `/rooms`, then
updates the encrypted shared token with a compare-and-swap. A simultaneous
administrator/contributor update wins. No application or reservation is submitted.

Store the authorized school account in `~/.config/cbgpu-eduset-refresh.env`:

```dotenv
EDUSET_USERNAME=your-school-username
EDUSET_PASSWORD=your-school-password
```

Use file mode `600`. Never commit credentials. The configured account must have authorized school lookup access; it may differ
from the contributor who supplied the previous shared token. The worker discovers the active web
release and its local database/encryption settings without duplicating secrets.

Install `refresh.mts` into `~/.local/lib/cbgpu-study-monitor/`, install both unit
files into `~/.config/systemd/user/`, then run:

```bash
systemctl --user daemon-reload
systemctl --user enable --now cbgpu-study-refresh.timer
systemctl --user start cbgpu-study-refresh.service
```

`node refresh.mts --check` checks configuration without attempting login.
`node refresh.mts --verify-login` verifies a real login without replacing the shared
token, including when renewal is not due. Failed attempts have a six-hour cooldown
which also survives worker crashes. A successful scheduled renewal removes the
cooldown. A human verification challenge is left to the operator; the worker does
not bypass it or discard the current token. The existing study monitor continues
to warn about expiry and actual lookup failures.

Status is written without credentials or tokens to
`~/.local/share/cbgpu-study-monitor/refresh-status.json`.
Run `node --test operations/study-refresh/refresh.test.mts` for renewal policy tests.
Disable automation with `systemctl --user disable --now cbgpu-study-refresh.timer`.

### Verification diagnostics

A 25-second wait without a normal response does **not** prove that a human was
asked to solve a challenge. The worker observes only aggregate SDK HTTP failures,
failed challenge requests, browser connectivity, page-error counts, widget-frame
presence, and whether the normal response field exists/is ready. It does not log
URLs, input values, cookies, credentials, or challenge responses, and it never
intercepts or modifies the site's network requests or verification callbacks.

`verification_network_failed`, `verification_browser_unavailable`,
`verification_page_failed`, and `verification_widget_unavailable` identify
observable technical failures. `verification_not_completed` means the normal
widget did not finish within the bounded wait; the cause may still require
legitimate operator action. It is not an automatic-renewal guarantee. The
six-hour retry cooldown, compare-and-swap token replacement, and read-only token
validation remain unchanged. Historical `human_verification_required` statuses
have no network evidence and cannot retroactively distinguish these causes.

Run `node --test operations/study-refresh/*.test.mts` for synthetic verification
and renewal-policy tests. Install `verification.mts` alongside `refresh.mts` when
an operator separately approves deploying this source revision; updating source
does not update an existing installed `.mjs` worker.
