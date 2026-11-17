# Deployment architecture

The implementation and measured limits are recorded in [cloud-readiness.md](./cloud-readiness.md).
The central application consists of the Next.js web/BFF, native Rust APIs,
PostgreSQL/Auth/Storage, persistent workers and private object/artifact storage.
The current host also runs compatibility services awaiting separately fenced
native handoffs. A source replacement does not switch the running process.

School GPU controllers, notebook kernels, model execution and school-owned volumes
remain at their schools. Enrollment is outbound, authenticated and private by
default. A central relocation must preserve protocol versions, reservation IDs,
job IDs and reconciliation; it must never infer sharing consent from enrollment.

Public and authenticated traffic retain the same stable domains. Host allowlists,
caller-bound session checks, tenant policies and file authorization are application
boundaries, regardless of whether ingress is Cloudflare or a cloud load balancer.
