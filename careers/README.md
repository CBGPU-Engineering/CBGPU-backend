# CBGPU Careers

Recruitment and team onboarding for CBGPU at **https://careers.cbgpu.com**.
Seven role descriptions, category/search filters, application forms, private PostgreSQL
storage, and a Slack / Jira / GitHub onboarding guide. This recruits school project
contributors; it does not promise employment, pay, acceptance, or a response deadline.

![Recruitment homepage](./docs/desktop.png)

<details><summary>Mobile and onboarding screenshots</summary>

<img src="./docs/mobile.png" alt="Mobile recruitment homepage" width="300">

![Team onboarding guide](./docs/onboarding.png)

</details>

## Run

The service is an Axum application backed by PostgreSQL. Build it from the
backend Rust workspace, then provide a private environment file (permissions `600`):

```bash
cargo build --locked --release --manifest-path rust/Cargo.toml -p cbgpu-careers
export CAREERS_DATABASE_URL=postgresql://SCOPED_USER:PASSWORD@127.0.0.1/DATABASE
export CAREERS_PUBLIC_DIR="$PWD/careers/public"
export CAREERS_ROLES_PATH="$PWD/careers/roles.json"
export CAREERS_FORM_KEY_PATH=/path/to/private/careers/form.key
export CAREERS_LISTEN=127.0.0.1:3530
export CAREERS_ORIGIN=https://careers.cbgpu.com
export CAREERS_ENABLE_OUTBOX=0
rust/target/release/cbgpu-careers serve
```

Apply `rust/migrations/20261116034058_rust_careers.sql` using the database owner
before starting the scoped runtime role. Preserve the existing 64-character form key.
Mail stays disabled in previews. Enable `CAREERS_ENABLE_OUTBOX=1` and set
`CAREERS_MAIL_ENV` to the existing private sending configuration only for the
single authoritative production writer. Never run two notification workers against
the same imported dataset during cutover.

Listen on loopback behind an HTTPS tunnel. Keep `CF-Connecting-IP` trusted only
from the Cloudflare tunnel; this service must not be exposed directly to the network.
The database, signing key, WAL files and applicant data belong outside Git.

## Review applications

Use the authenticated developers portal terminal or SSH. Load the same private
`CAREERS_DATABASE_URL` as the service, then:

```bash
rust/target/release/cbgpu-careers --list
rust/target/release/cbgpu-careers --show CB-YYMMDD-REFERENCE
rust/target/release/cbgpu-careers --show CB-YYMMDD-REFERENCE --status contacted
rust/target/release/cbgpu-careers --delete CB-YYMMDD-REFERENCE
```

Review submitted links as untrusted user content. The service never fetches them.
There is deliberately no public application listing or applicant search endpoint.
Application bodies are not logged. Save the reference when requesting changes.

## Privacy and delivery

Applicants explicitly consent to 90-day retention; expired applications are deleted
by the minute maintenance worker, including when notification credentials are absent.
Private filesystem permissions, same-origin writes, signed expiring form tokens,
idempotent submission, request size/field bounds, a honeypot and persistent per-IP
rate limits protect reception. Hashed IP counters expire after 24 hours.
Notifications contain only role/reference and go to `info@cbgpu.com`. Delivery is
retried every minute, with a provider idempotency key; a failed email never discards
an application. `notified=1` records successful provider acceptance, not inbox delivery.

Slack/Jira links come from the team owner. Invites may expire and do not grant GitHub
organization access automatically. Onboarding requires joining Slack, signing up for Jira, and posting a GitHub ID and Jira signup email in Slack so the team can invite each account manually,
followed by the team's explicit organization invitation. Checklist state is not saved.

## Content and checks

Edit `roles.json` for role content and `public/app.ts` for the joining guide. Styling
uses CBGPU assets and original copy. Toss recruiting information architecture was
referenced, without reusing its branding, assets or job descriptions.

```bash
cargo test --locked --manifest-path rust/Cargo.toml -p cbgpu-careers
CBGPU_CAREERS_RUN_DOCKER_TEST=1 cargo test --locked --manifest-path rust/Cargo.toml -p cbgpu-careers --test careers_postgres -- --ignored
node --check careers/public/app.js
```

Apache-2.0, as the enclosing backend repository.

## Publish the hostname

Add a published application route to the existing Cloudflare Tunnel:
`careers.cbgpu.com` → `http://127.0.0.1:3530`, with the Path field empty.
Public TLS terminates at Cloudflare; the origin is loopback HTTP.
`cbgpu-routing careers --apply` performs the same additive change when
the existing Cloudflare routing authentication is valid. It checks for DNS
conflicts and snapshots the tunnel before making changes.

The joining guide is an independent server-rendered page at `/onboarding`;
it does not need the role API or application JavaScript to display its content.
Edit `public/onboarding.html` for this dedicated page.
`onboarding.js` only adds clipboard behavior.

For unattended routing, save a scoped API token (Account → Cloudflare Tunnel →
Edit and Zone → DNS → Edit, limited to the CBGPU account/zone) in
`/home/cbgpu/.config/cbgpu/cloudflare-routing-token`, permissions `600`.
The route script prefers that credential over an interactive dashboard session.
Never put the token in source files or chat. An alternative token path can be
passed through `CBGPU_CLOUDFLARE_ROUTING_TOKEN_FILE`.

## Shared public-site appearance

Recruitment, job details, application forms and the standalone joining guide use
CBGPU main-site light/dark colors from `src/app/globals.css`, the system font
stack, grayscale logo, italic wordmark, neutral bordered cards and primary button
conventions. The header theme toggle follows the main site's `theme` storage key
and system default. Preferences persist within this origin; localStorage is not
shared between subdomains. No separate font service is needed.

![Dark appearance](./docs/desktop-dark.png)

The onboarding guide is distributed directly to accepted applicants. It has no
public navigation buttons, is excluded from the sitemap and includes a noindex
directive. Direct links remain accessible; this is not an authentication gate.

## Recruitment roles

Android and iOS are independent application tracks; both contribute to the shared
Flutter/Dart codebase. iOS includes Xcode/device testing, platform integration and
TestFlight/App Store preparation; Swift experience is preferred, not mandatory.
Backend owns service APIs, authorization, data models and application logic.
Platform / infrastructure owns the deployment product, containers, GPU resource
management, project domains, logs and operations. Contributors may collaborate
across these roles without collapsing the application tracks.

Old `/roles/mobile` and `/apply/mobile` links return to the role list so applicants
can choose a platform. Existing `mobile` applications retain their original role;
notification delivery still supports that historical label.
