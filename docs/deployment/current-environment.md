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
- Web release `20cd21420ddb947699bba0e7022e74b4378f7283` is live, with exact
  health SHA verified and GitHub main updated through a normal linear push. Its
  source tree matches the preceding `e3` release. Android downloads remain outside
  each immutable web release and were preserved through release packaging.
- The `770207c` school timetable warm cron ran successfully against real school
  data: 34 cells across five days. Its private credential-file setting must remain
  server-only and survive subsequent delivery.
- The delivery unit uses the reviewed `9971a9c` native binary, including warm
  environment preservation. Web state adopted `20cd` without changing backend
  delivery state. The watcher has resumed (`paused=false`) and its timer is active.
- The LaTeX gateway-only transition is live (`e03` runtime, source `9db29f6`).
  The reviewed freeze verified all old gateway threads stopped and zero TCP
  connections/receive queues before replacing only the idle gateway. Mongo
  retained seven projects, nine users and matching ID/owner fingerprints. Compiler
  PID `3095423` was untouched. New-school LaTeX compilation remains denied by the
  explicit CBsh resource policy; a live gateway is not a cross-school compute grant.
- The operating immutable Python AI service and root GPU controller remain
  unchanged. Scoped GPU execution and external GPU sharing remain pending.
  No AI, GPU, CE, Mongo or Redis restart or notebook-kernel reset occurred.
- PostgreSQL, SQLite handoff sources, uploads, material bytes, project artifacts,
  encryption/signing keys and required configuration are persistent data.
  Email, GitHub, Firebase and school providers remain external dependencies.

See [activation record](./activation-review.md) and
[readiness](./migration-readiness.md) for evidence and remaining boundaries.
This activation is not a claim that every runtime is Rust, every workflow has
passed an operating-user test, dependencies are vulnerability-free, or the
system has moved to AWS.
