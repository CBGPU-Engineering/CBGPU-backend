# Authorized native AI corpus — candidate

This library replaces the legacy shared-material/published-paper SQLite index and
Python extraction worker. It is **not activated**, does not apply migrations on
startup, and does not import production data automatically. It never indexes
private research notes. Notes/docs continue to use the caller-authorized text
already supplied to the native search model worker.

`Corpus` receives two private pools: the trusted runtime AI database and a narrowly
provisioned application policy-oracle connection. The owner comes from an active
`ai_store.jobs` search record selected by the scheduler, not input JSON, headers,
filesystem metadata, candidate labels or command-line user IDs. An HTTP caller
must never choose another user's scheduler job ID. This crate exposes no HTTP
endpoint.

For each trusted search job:

1. Intersect its stored `corpus_ids` with its stored candidate IDs. These are hints,
   not authorization.
2. Ask the private application oracle to evaluate existing source RLS under the
   job owner's fresh identity. Deleted/banned/suspended identities fail closed.
3. Read only those cached sources at their current source/model revision.
4. Rank normalized E5 vectors plus the existing 80/20 lexical component.
5. Recheck the job and source permissions/version before returning snippets and
   fresh server-derived labels/links. Membership/sharing/publication revocation
   excludes previously cached text.

The oracle is owned by a non-login, non-superuser, non-bypass-RLS reader that owns
no tables and inherits only `authenticated`. The outer function is executable
only by a separate trusted backend role. Browser roles cannot call it, choose an
owner, read private chunks or invoke the inner policy helper. Published preprints
are projected explicitly because their existing Data API is intentionally closed.
The corpus reader receives no auth-table or source write grants.

Legacy material profile approval remains in force. `Corpus::new` defaults account
V2 **off**. `with_account_v2(true)` is only for an explicitly approved rollout;
that narrow reader may then preserve an owner's material reads after graduation
or reverification, and delegates other shared-storage access to the existing
fresh `cbgpu_can_use_resource('school_storage','cbsh')` oracle. Missing/expired
grants fail closed. An unapproved old account is never assumed to be a rollover.

## Extraction and cache

`ingest(job, key, files, embedder)` rechecks authorization before opening files and
after embedding. `SourceFiles` is configured only by the trusted operator:

- Materials use canonical UUID hex/shard storage mapping, not editable filenames
  or arbitrary metadata paths. Unix opens each component with `openat` and
  `O_NOFOLLOW`; symlinked shards/files and non-regular objects fail closed.
- Paper PDFs come only from a fixed loopback published-paper endpoint with a fixed
  Host header, no service-role token, no redirects and current size/SHA checks.
  Remote/user URLs are rejected.
- Prefer `SourceFiles::sandboxed` with an explicitly reviewed **local image
  digest**, not a mutable/user-selected tag. It invokes native Poppler directly
  in a disposable CPU-only, read-only, network-none container with no capabilities,
  no-new-privileges, unprivileged UID, PID/memory/CPU/tmpfs bounds, no image pulling
  and a 180-second deadline. No Python entrypoint runs. Cleanup affects only its
  unique `cbgpu-corpus-*` container.
- An explicit absolute native `pdftotext` path is supported for provisioned hosts;
  Linux applies native `prlimit`. The current host has no `/usr/bin/pdftotext`, so
  it must use the reviewed sandbox image or provision native Poppler first.

Supported legacy formats remain PDF/TXT/MD/CSV/TEX/PY/LOG. The file-content budget
is unchanged: 200 MB material / 21 MB paper, first 100 PDF pages / 200,000 Unicode
characters; 650-character chunks at 500-character stride. These are corpus
extraction bounds, not changes to website upload quotas. Image-only PDFs retain
metadata search until a separate OCR ingestion policy is reviewed.

The optional `e5` feature uses the existing native Candle E5 library explicitly
on **CPU**, with its existing `passage:` / `query:` prefixes and pinned revision.
No GPU reservation or assignment is touched. `Embedder` lets deterministic fake
vectors test policy/ranking without public weights or hardware.

## Database and import

- Application candidate:
  `supabase/migrations/20261116142026_ai_corpus_policy_reader.sql` (repository root).
- Separate runtime database candidate: `migrations/runtime.sql`.

Apply neither to production without backup, isolated tests and explicit approval.
Provision private login credentials separately; do not grant these functions to
anon/authenticated or expose the runtime database through PostgREST.

`importer::import_legacy` opens an explicit regular SQLite cache read-only, keeps a
consistent read transaction, validates IDs/states/dimensions, and copies exact
source versions, chunk numbers, text and vectors into one PostgreSQL transaction.
Existing native documents win; repeated import is additive/idempotent. The old
file is never edited, deleted or renamed. Imported text still passes the same
fresh visibility/version checks before retrieval. This disposable index is not a
project/user migration.

## Tests

```text
cargo test --manifest-path Cargo.toml
cargo clippy --manifest-path Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path Cargo.toml --test postgres -- --ignored
```

The ignored PG test creates only its own network-none PG17 Docker fixture and
synthetic files; no production URL is accepted. It tests owner/member/private/
public reads, forged IDs/owner metadata, current labels, legacy approval versus
explicit V2, revocation during ranking, withdrawal, bans/cancellation, role grants
and exact idempotent legacy import. Synthetic visibility/member policies exist
only in that fixture, not the real application migration.

Optional ignored tests use an explicit `CBGPU_CORPUS_TEST_OCR_IMAGE` local image
digest for a disposable synthetic PDF container, and `CBGPU_CORPUS_TEST_E5` pointing
to public pinned weights with `--features e5` for an actual CPU embedding smoke.

## Integration still required

The root scheduler/model worker must merge these authorized full-text hits with
existing caller-text ranks, and call lazy ingestion on authorized cache misses.
Keep the query/candidate metadata path for unsupported files. Do not reenable
legacy unrestricted `corpus_ids` lookup or expose cache APIs directly to clients.
Source deletion/invalidation hooks and background refresh remain to be wired by
the host integration; fresh retrieval already denies removed/withdrawn/revoked
sources. Existing Python sources/runtime copies are retained pending reviewed
integration, approved schema rollout and controlled service cutover.
