# Security policy

## Supported versions

The latest code on `main` and the latest non-prerelease version are maintained.
Older APKs and historical deployments do not receive separate backports. A tag
is a source snapshot, not a guarantee that every optional integration is supported.

## Report privately

Use [GitHub's private vulnerability reporting](https://github.com/CBGPU-Engineering/CBGPU-backend/security/advisories/new).
If it is unavailable, email **lmw.hpc@gmail.com** with the subject
`[CBGPU security]`.

Include the affected version or commit, component, expected behavior, reproduction
steps and impact. Use synthetic examples. Do not include passwords, live session
cookies, student records, signing keys or private documents. We will arrange a
private way to receive sensitive evidence if necessary.

Do not publish an exploit or sensitive details in a public issue before discussing
disclosure with maintainers. Acknowledgment and fixes are best effort; we cannot
promise a fixed SLA or a monetary bounty.

## Testing boundaries

Test your own local installation or the synthetic demo. Public source access does
not authorize probing the school's live systems, other users' projects or the
underlying EduSet service. Do not submit real applications, run equipment, send
mail/push, enumerate accounts or stress production as part of security testing.

## Maintainer checklist

Reproduce in isolation; determine affected versions; prepare and test the fix;
coordinate disclosure; publish an advisory and patched release when appropriate.
Rotate compromised credentials through their issuer. Do not place evidence or
credentials in commits. Review dependency alerts and keep security fixes separate
from unrelated upgrades where possible.

## Bundled SQLite compatibility patch

The Rust workspace uses SQLx 0.9, rusqlite 0.41.1 and bundled SQLite 3.54.1.
The official SQLx SQLite driver currently limits the SQLite binding below the
patched version. `rust/vendor/sqlx-sqlite-0.9.0` therefore preserves the official
driver's Rust sources and both licenses byte for byte, adapting only its manifest
to the exact registry `libsqlite3-sys` 0.39.1 with column metadata enabled.
`node scripts/check-vendored-sqlite.mts` verifies that boundary offline; the
provenance file records the official archive checksum. No SQLite C source is
vendored here. Remove this manifest adaptation when an upstream release supports
the patched binding. The workspace requires Rust 1.94 or later.
The upstream library's unused lockfile is excluded; the application's reviewed
workspace lockfile determines all dependencies.

See [the SQLite advisory](https://rustsec.org/advisories/RUSTSEC-2026-0459.html)
and [the RSA advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html).
RSA signing uses the patched 0.10 API; existing signing keys are retained.

Dependency scans cover source dependencies, not an assurance that an entire
deployment is vulnerability-free. The November 18, 2026 Rust lockfile scan has
no reported vulnerabilities and still reports unmaintained dependencies.
The cloud-preparation toolchain separately has two moderate npm findings for
the pinned CDK's bundled minimatch. A normal npm override does not replace that
bundled copy; no ineffective override or advisory suppression is checked in.
These checks create no AWS resources; live AWS deployment requires a separate
infrastructure review.
