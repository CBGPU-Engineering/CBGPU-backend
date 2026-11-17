# Current environment

This is an operating snapshot dated 2026-11-18, not an automatically maintained
inventory. Recheck the running binary, configuration and listener before another
cutover. Never copy secret values into this document.

- Central services remain on the existing school computer behind Cloudflare TLS.
  No AWS migration or paid cloud infrastructure was activated.
- Database: `supabase/postgres:17.6.1.136`. The reviewed 47-migration core bundle
  committed atomically. Validation preserved 175 Auth users, 167 profiles and
  175 historical CBsh memberships; 146 eligible legacy users have an account-owned
  school selection. The official directory contains 2,408 schools. The one trusted
  teacher was preserved; GPU sharing and academic rollover counts remain zero.
  Auth, Storage, RLS and custom roles remain dependencies. `supabase_admin` and
  `postgres` have different privileges.
- Native Schools API `3546`, public content/assets API `3547`, ad report API
  `3549` and the school-mail worker are started with their reviewed enabled
  configurations. Public assets and mail use separate narrow database logins;
  provider acceptance must not be described as inbox delivery.
- Deployment control is the scoped Node.js compatibility release `1b70506`.
  Materials release `2d9` is live with scoped authorization. Existing files,
  encryption keys and history were preserved; materials admission is unfenced.
  This does not establish a completed Rust deployment-control handoff or S3 move.
- Web release `e3` is serving verified public HTTPS traffic at this snapshot.
  Git main `20cd` is the linear reconciliation with the same exact source tree;
  its final immutable build is pending. Android downloads remain outside each
  immutable web release and must be preserved through release packaging.
- The `770207c` school timetable warm cron ran successfully against real school
  data: 34 cells across five days. Its private credential-file setting must remain
  server-only and survive subsequent delivery.
- The delivery unit now points to the reviewed `9971a9c` native binary, including
  warm-environment preservation. The watcher remains paused pending final web
  SHA adoption and verification; this snapshot does not claim it has resumed.
- The operating immutable Python AI service and root GPU controller remain
  unchanged. Scoped GPU execution, external GPU sharing and the first LaTeX
  gateway transition remain pending. Existing notebook kernels were not reset.
- PostgreSQL, SQLite handoff sources, uploads, material bytes, project artifacts,
  encryption/signing keys and required configuration are persistent data.
  Email, GitHub, Firebase and school providers remain external dependencies.

See [activation record](./activation-review.md) and
[readiness](./migration-readiness.md) for evidence and remaining boundaries.
This activation is not a claim that every runtime is Rust, every workflow has
passed an operating-user test, dependencies are vulnerability-free, or the
system has moved to AWS.
