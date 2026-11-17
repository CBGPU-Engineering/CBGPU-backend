# Official ad revenue reporting

Source candidate for a private Rust administrator API on `127.0.0.1:3549`.
`CBGPU_AD_REVENUE_ENABLED` defaults off. Enabling it requires the reviewed SQL
migration and an existing backend origin plus public anon key; callers supply
an authenticated Bearer JWT. Every operation checks a fresh platform-admin
session in PostgreSQL. School administrator status alone grants no access.

GET `/v1/admin/ad-revenue/analytics` accepts explicit `from`, `to`, `kind` and
`timezone`. POST `/preview` and `/imports` accept UTF-8 CSV with explicit header,
date, delimiter and number mappings. Imports require the returned normalized
`preview_hash` and `confirmed:true`; the server reparses and hashes the report.
Limits are 2 MiB CSV, 5,000 rows, 128 columns and 3 MiB JSON. No official CSV
format is guessed. Revenue is stored as integer millionths of KRW; counts are
optional exact integers. Missing observations remain null, distinct from zero.

Estimated and confirmed reports are separate. Reimporting identical normalized
facts is idempotent; different overlapping observations are rejected atomically.
There is currently no correction/superseding workflow. Raw CSV is not retained;
source and normalized SHA-256 hashes record provenance. These hashes do not
prove that an uploaded report came from the provider.

Analytics includes actual daily observations and wholly contained monthly
observations, with coverage explicitly unverified. Partial periods are excluded;
cross-month period totals are never allocated to invented daily/monthly values.
Daily unique visitors are never summed into monthly visitors. RPUV/RPS are N/A
without aligned aggregate denominators. Fill rate is N/A because request and
impression semantics have not been verified. eCPM/CTR use only supplied official
counts. The desktop `DAN-kVpBmlPRbOvGb6oi` and mobile
`DAN-vpZNfcj3tj0jMkhV` units identify mobile/desktop banner formats, not physical visitor devices. A
narrow desktop window can use the mobile-format unit. DTOs explicitly return
`device_basis: AD_UNIT_FORMAT`; physical-device revenue is N/A with reason
`NO_OFFICIAL_DEVICE_DIMENSION`. The units are shared across pages, so
per-placement revenue is N/A.

`/experiments` records planned 60/120-second console settings and explicit
administrator attestations with optimistic revisions. It changes no advertising
SDK timer and reports no winner, provider verification or causal attribution.
No official-console API integration is implemented.

`/health` is liveness only, not database readiness. The loopback listener supports
the documented shared-network ingress-sidecar deployment; this source does not
create cloud resources or activate any service/schema.

Validation:

```sh
cargo test --manifest-path rust/Cargo.toml -p cbgpu-ad-revenue
cargo test --manifest-path rust/Cargo.toml -p cbgpu-ad-revenue --test postgres -- --ignored
cargo clippy --manifest-path rust/Cargo.toml -p cbgpu-ad-revenue --all-targets -- -D warnings
```

The ignored integration fixture uses disposable network-isolated PostgreSQL 17,
not operating reports or registrations. The school schema dry-run also verifies
this migration twice against a same-image operating-schema clone and rolls back
synthetic reporting assertions.
