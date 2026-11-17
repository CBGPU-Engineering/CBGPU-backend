# Rollback boundaries

Before the new writer starts, stop/fence the candidate and restore the recorded
original listener/configuration. Verify the original database/file source has
not changed and never restore a whole-host firewall snapshot over Docker rules.

After new writes occur, **DNS reversal is not a data rollback**. Freeze both
writers, drain/reconcile jobs and copy the authoritative database/file delta back
through a rehearsed reverse migration. Include new sessions, memberships,
submissions, uploads, deletions, environment changes and GPU job state. Verify
hashes and references before reopening the old writer.

Native deploy tooling supports a new, authenticated reverse-export destination;
it does not overwrite the original SQLite database. Verify its encrypted manifest
and activation receipt with the corresponding compatibility service. AI has a
separate handoff/reverse contract. Do not reuse deploy receipts for AI or treat a
closed port as proof all workers stopped.

If reverse synchronization is unverified, retain maintenance mode and recover
forward from the authoritative source. Never discard newer user data to make an
old process start. GPU kernels and project volumes must survive routing rollback.
