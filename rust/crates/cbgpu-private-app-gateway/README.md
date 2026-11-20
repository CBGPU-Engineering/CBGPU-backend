# Private local application gate

This HOST process protects one local-only, unauthenticated application. It must **never** run inside that application's untrusted container. The application receives neither CBGPU credentials nor Studio cookies. A separate credential-free `studio-bridge` binary supervises the application's supported loopback command and forwards raw TCP inside its container.

Deployment layout:

```
existing HTTPS project gateway -> HOST 127.0.0.1:9000 (trusted SSO gate)
  -> HOST 127.0.0.1:19000 -> container :8786 (raw bridge)
  -> container 127.0.0.1:8787 (unchanged local application)
```

The private, mode-0600 configuration contains `listen`, `upstream`, `app_authority`, `host`, and `project`. All network addresses must be loopback in the host gate. `host` is an exact project subdomain, not a suffix allowlist. `app_authority` is the application's own loopback address, used only after authorization for its existing local Host/Origin guard.

Anonymous GETs create a bounded, five-minute challenge and a host-only nonce cookie, then redirect to the fixed first-party `/server/{id}/open` route. Exact `app.cbgpu.com` referrers and known app WebView agents retain the app first-party UI; all other inputs use `cbgpu.com`. No caller-controlled return origin is accepted. Anonymous unsafe requests return 401.

The first-party BFF sends the explicit caller Bearer token and challenge to `POST /_cbgpu/auth/tickets`. Fresh authority comes from HOST `http://127.0.0.1:3000/api/server/{id}/open`, with Host `cbgpu.com`; it must validate the actual account, active session, current project and owner/editor permissions. It returns `{allowed:true,userId,role}`. Viewer access is denied. The returned 30-second, single-use ticket is bound to the original browser nonce. Redemption creates an opaque host-only `__Host-studio-session` cookie and redirects to the stored safe relative path. No CBGPU token appears in a URL, browser cookie, log or application request. Maps and challenge/ticket issuance rates are bounded; a gate restart invalidates all grants.

Every application request revalidates its user and permissions. Uploads, response streams and SSE revalidate at five-second intervals and close on revocation or authority failure. Connect and authority-check timeouts are bounded; application requests do not have an artificial total deadline. Bodies stream in both directions. Unsafe application requests require the exact HTTPS project Origin. WebSockets are unsupported and rejected. All application Set-Cookie headers are dropped, reserving the gateway's authentication cookies; identity headers and cookies are stripped before forwarding.

## Managed project redeployment contract

The project's canonical database `host_port` remains 9000, pointing at the trusted gate, **never** at the raw application. Only the failed project's container publication changes to `127.0.0.1:19000`; all existing application data, environment, volumes and limits must be preserved. The raw port must never bind a LAN/public address.

The HOST `.socket` unit owns `127.0.0.1:9000` independently of the gate process, including during gate restarts. The binary accepts only a matching systemd inherited socket. A future generic controller attempting to publish `0.0.0.0:9000` must fail on this existing bind; it must not stop the host gate/socket or bypass authentication. Generic redeployment of this protected project requires a managed, reviewed plan preserving this boundary. No platform-wide controller activation is part of this crate.

Before changing the failed deployment, snapshot its complete configuration privately, retain the old stopped container and existing volumes, and record reversible port/command changes. Restore by stopping only the replacement, restoring the preserved container and original port publication, and stopping the gate/socket only when rollback intentionally reclaims that port. Never prune the original generated-output recovery backup or application data.

The bridge corrects only the deployment's unsupported launch flags. Local OAuth callback assumptions and third-party application behavior are unchanged; they require separate application review rather than silent proxy rewrites.
