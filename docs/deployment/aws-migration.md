# Future AWS migration runbook

**Do not execute this runbook without a separately reviewed deployment approval.**

1. Freeze the reviewed source/image digests and record operating service versions,
   role/extension requirements, credentials references and current health baseline.
2. Take encrypted database, files, keys, configuration and persistent-job backups;
   verify hashes and restore them into an isolated same-version environment.
3. Prepare approved AWS staging and deploy the same image digests with different
   runtime connection settings. Do not expose staging to production writers.
4. Rehearse users/sessions, tenant isolation, public/private API responses, file
   authorization, provider callbacks, SSE/WebSocket and daemon reconnection.
5. Rehearse final synchronization and rollback, including data written after the
   switch. Record acceptable outage and recovery objectives from actual tests.
6. At the approved window, freeze affected writes and drain persistent queues.
   Fence every old writer, including orphan workers; a closed listener alone is
   insufficient. Preserve running school kernels and original volumes.
7. Synchronize final database/file/job deltas and verify object hashes, ownership,
   membership, session validity and job identities. Never run both schedulers.
8. Start the approved target writer, switch ingress/DNS, verify daemon state and
   perform non-destructive login/project/contest/GPU/AI smoke checks.
9. Observe errors, latency, queues, permissions and costs. Preserve the old source
   read-only until the recovery window closes; do not delete it automatically.

If validation fails, use [rollback.md](./rollback.md). Staging behavior does not
prove AWS production networking, IAM or provider delivery until actually tested.
