#!/usr/bin/env bash
set -euo pipefail
umask 077
backend_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
backup_dir="$backend_dir/backups/$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$backup_dir"
docker exec cbgpu-supabase-db pg_dump -U supabase_admin --format=custom postgres > "$backup_dir/database.dump"
docker exec cbgpu-supabase-db psql -X -U supabase_admin -d postgres -At -c "SELECT json_build_object('users',(SELECT count(*) FROM auth.users),'profiles',(SELECT count(*) FROM public.profiles),'public_tables',(SELECT count(*) FROM information_schema.tables WHERE table_schema='public'),'files',(SELECT count(*) FROM storage.objects));" > "$backup_dir/snapshot-counts.json"
docker exec cbgpu-supabase-db pg_dumpall -U supabase_admin --roles-only > "$backup_dir/roles.sql"
docker exec cbgpu-supabase-storage tar -czf - -C /var/lib/storage . > "$backup_dir/storage.tar.gz"
docker exec cbgpu-supabase-db tar -czf - -C /etc/postgresql-custom . > "$backup_dir/database-encryption-keys.tar.gz"
tar -czf "$backup_dir/materials.tar.gz" --exclude="*.partial" --exclude="*.upload.json" -C /home/cbgpu/cbgpu-materials .
cp "$backend_dir/.env" "$backend_dir/functions.env" "$backup_dir/"
node "$backend_dir/scripts/backup-developer.mjs" "$backup_dir"
sha256sum "$backup_dir/database.dump" "$backup_dir/storage.tar.gz" > "$backup_dir/checksums.sha256"
printf 'Backend backup saved: %s\n' "$backup_dir"
node "$backend_dir/operations/backup.mjs" upload
