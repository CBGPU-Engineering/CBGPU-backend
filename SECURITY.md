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
