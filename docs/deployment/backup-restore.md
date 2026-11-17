# Backup and restore

The native `cbgpu-backup` source captures application/runtime PostgreSQL dumps,
roles, Storage bytes, encryption keys, materials and helper-managed configuration
and developer state. It retains partial failures privately and hashes outputs.
Review its configured runtime/helper roots and offsite encryption configuration
before running it; defaults still include compatibility host paths.

The school schema dry-run binary performs an explicit read-only operating dump
and restores it into a network-isolated disposable container using the same
PostgreSQL image. It applies additive migrations twice and compares historical
Auth/password/session and business-table fingerprints. Private reports stay
outside Git. It never applies these migrations to the operating database.

A schema-clone pass verifies the database rehearsal, not a complete disaster
recovery. Restore Storage/material bytes, configuration and encryption/signing
keys separately, preserve object paths and verify content hashes and API-level
owner/collaborator/denied reads. Test queued jobs without executing duplicates.

Retain originals until recovery has been demonstrated. Backups contain personal
information and secrets: encrypt them, restrict access, define a reviewed retention
schedule and test recovery keys. No offsite upload or full production restore was
performed as part of the cloud-readiness checks.
