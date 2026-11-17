# Runtime configuration

Use component `.env.example`/container examples as the authoritative option lists;
never commit real `.env`, database URLs, cookies, API keys or signing material.

| Boundary | Configuration and constraints |
| --- | --- |
| Web public build | `NEXT_PUBLIC_SUPABASE_URL`, public anonymous key and public feature flags; never a service-role key. Keep stable public domain when relocating. |
| Web internal proxy | `CBGPU_BACKEND_PROXY_URL`, `SUPABASE_INTERNAL_URL`, `CBGPU_INTERNAL_NETWORK_MODE`; trusted internal destinations only. |
| Session oracle | `CBGPU_SESSION_ORACLE_MODE=rpc` requires the installed self-session RPC. Missing/outage fails closed; no Docker fallback in scoped mode. |
| Completed Android files | `CBGPU_ANDROID_RELEASE_PATH`; mount canonical release metadata and completed downloads read-only. |
| School APIs | `CBGPU_SCHOOLS_API_URL`, `CBGPU_ACCOUNTS_API_URL`, `CBGPU_PUBLIC_CONTENT_API_URL`; configure corresponding server/public flags together after verification. |
| Authorization | `CBGPU_MULTISCHOOL_V1`, `CBGPU_ACCOUNT_V2`; canonical enabled `1`/`true`, disabled `0`/`false`. Do not trust public flags as server authority. |
| Native persistent state | Component database/storage configuration and explicit import receipts; never infer a safe writer handoff from an environment flag. |
| Email/providers | Private provider keys, sender/callback settings and existing encryption keys; keep stable identities and verify provider connectivity. |
| Browser helper | Baked `CBGPU_CHROMIUM_EXECUTABLE_PATH`; sanitized child environment excludes application credentials. |

Supply secrets through protected files now or task/instance secret references
later. CDK examples reference existing secret ARNs, not plaintext values. Runtime
configuration changes are deployment actions and require the same review as code.
