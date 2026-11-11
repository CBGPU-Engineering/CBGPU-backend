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

Use file mode `600`. Never commit credentials. The configured account must match
the identity of the current shared token. The worker discovers the active web
release and its local database/encryption settings without duplicating secrets.

Install `refresh.mjs` into `~/.local/lib/cbgpu-study-monitor/`, install both unit
files into `~/.config/systemd/user/`, then run:

```bash
systemctl --user daemon-reload
systemctl --user enable --now cbgpu-study-refresh.timer
systemctl --user start cbgpu-study-refresh.service
```

`node refresh.mjs --check` checks configuration without attempting login.
`node refresh.mjs --verify-login` verifies a real login without replacing the shared
token, including when renewal is not due. Failed attempts have a six-hour cooldown
which also survives worker crashes. A successful scheduled renewal removes the
cooldown. A human verification challenge is left to the operator; the worker does
not bypass it or discard the current token. The existing study monitor continues
to warn about expiry and actual lookup failures.

Status is written without credentials or tokens to
`~/.local/share/cbgpu-study-monitor/refresh-status.json`.
Run `node --test operations/study-refresh/refresh.test.mjs` for renewal policy tests.
Disable automation with `systemctl --user disable --now cbgpu-study-refresh.timer`.
