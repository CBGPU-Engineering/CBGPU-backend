# Portal public asset compatibility

The developer and careers portals use the approved transparent black CBGPU logo.
Dark mode converts that existing PNG to white with `brightness(0) invert(1)`.
It does not depend on adding SVG routes to the existing Node gateway or native
careers binary.

The developer Usage block is optional when new browser code encounters an older
HTML shell. Only its content and metadata lookups are optional; authentication,
CSRF, logout cleanup and required tool DOM remain unchanged. New onboarding code
keeps its live feedback region when available and falls back to the existing copy
button when it encounters the older HTML.

Build browser assets through `npm run build:browser`. Keep complete HTML, CSS and
JavaScript sets together when preparing an asset release; compatibility guards
also handle an older document that remains open during a release.

## Offline browser verification

No browser automation dependency is added to the application. Supply an existing
Playwright installation and read-only directories containing the legacy **public
assets** to run the optional compatibility check:

```sh
node scripts/verify-portal-asset-compat.mts \
  --playwright-module /path/to/playwright/index.mjs \
  --legacy-developer-dir /path/to/developer/static \
  --legacy-careers-dir /path/to/careers/public \
  --out /tmp/portal-asset-compat
```

Every request is intercepted and fulfilled from public files or synthetic JSON.
The check exercises new HTML/new script, old HTML/new script, and new HTML/old
script for both services. It verifies existing PNG routes and transparent black
logo pixels, no SVG requests, clipboard success and denial, and new-script
sign-in/overview/CSRF logout and DOM cleanup. It does not invoke legacy developer
logout because the audited older script has call sites without a `signOut`
implementation. No real account, database, terminal, email or external post is used.
