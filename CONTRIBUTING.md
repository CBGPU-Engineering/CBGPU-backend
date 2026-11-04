# Contributing to CBGPU-backend

This repository owns one component of [CBGPU Engineering](https://github.com/CBGPU-Engineering).
Read [README.md](README.md) for its boundary and local setup. To run the complete
system, use [CBGPU-platform](https://github.com/CBGPU-Engineering/CBGPU-platform) with recursive submodules.

1. Branch from `main`, make a focused change and run the checks below.
2. Submit a PR in this repository with the problem, resulting behavior and checks
   actually run. Use synthetic data in screenshots and logs.
3. After merging a component change, update its pinned submodule commit in the
   platform (or backend, for LaTeX) through a separate reviewed PR.

```bash
cd developer && npm ci && cd ..
node scripts/verify-mail-queue.mjs
```

Write project documentation and PR summaries in English. Existing Korean product
UI is intentional. Do not commit credentials or runtime data. Tests must not send
real emails, submit school applications, run equipment or touch production data.

Original contributions use Apache-2.0; preserve all third-party notices.
Report vulnerabilities privately using SECURITY.md. Participation follows
CODE_OF_CONDUCT.md; support and reviews are best effort.

Dependency version PRs are grouped monthly with at most one open version PR per
ecosystem. Dependency-only changes do not automatically request a code-owner
review. Security updates remain enabled and are not delayed by the version-update
schedule; maintainers still review changes and CI before merging.
