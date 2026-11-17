# Migration readiness

**Overall: not ready for an unreviewed production relocation. No AWS migration has
been performed.** Evidence is component-specific, not a blanket certification.

| Check | Evidence / remaining boundary |
| --- | --- |
| Database preservation | Same-image isolated schema clone applied additive DDL twice; historical accounts/sessions/business data preserved. No production DDL. |
| Native API packaging | Non-root read-only review container and real health endpoint tested. |
| Web packaging | Same image tested with two synthetic internal upstreams, read-only UID 65532, HTTP/APK/SSE/WebSocket and graceful shutdown. |
| Web vulnerability review | Reviewed distroless candidate scan found no High/Critical; Medium/Low/Unknown remain. Bundled browser needs its own continuing security review. |
| Materials storage | Real isolated S3-compatible forward/reverse/hash/corruption tests pass; no operating files copied. This is not an Amazon S3 IAM test. |
| Infrastructure source | Offline CDK assertions/synthesis and Compose parsing; no deployed AWS resources. |
| AI/deploy state | Source imports and reverse contracts tested; operating writer handoffs remain separate. |
| GPU compatibility | Scoped source and read-only preflight exist; active writers/direct ingress block activation. Existing kernels remain untouched. |
| Multi-school activation | Candidate flags remain off until every consuming service enforces fresh scopes. |
| Full recovery | Database rehearsal is not full file/key/provider recovery; complete staging restore still required. |

Real AWS tests must cover IAM/task-role credentials, private networking, provider
outbound access, storage multipart behavior, TLS/ingress, recovery, operating
latency/cost and daemon reconnection. Follow [aws-migration.md](./aws-migration.md)
only after explicit approval and a concrete reviewed cutover plan.
