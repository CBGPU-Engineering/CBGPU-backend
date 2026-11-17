# Current environment

This is a source review snapshot dated 2026-11-17, not an automatically maintained
inventory. Recheck the running image/binary, configuration and listener before any
cutover. Never copy secret values into this document.

- Central service remains on the existing school computer behind Cloudflare TLS.
- Database: `supabase/postgres:17.6.1.136`; Auth, Storage, RLS and custom roles are
  dependencies. `supabase_admin` and `postgres` have different privileges.
- Web releases are immutable; completed Android downloads live outside each release.
- Some native candidates coexist with operating compatibility services. In
  particular, old AI and GPU writers must remain fenced from simultaneous native
  ownership, rather than being stopped merely because the source was converted.
- PostgreSQL, SQLite handoff sources, uploads, material bytes, project artifacts,
  encryption/signing keys and required configuration are persistent data.
- Email, GitHub, Firebase and school providers remain external dependencies.

See [readiness](./migration-readiness.md) for evidence and remaining gaps. No AWS
migration, production school migration or nationwide GPU sharing was performed.
