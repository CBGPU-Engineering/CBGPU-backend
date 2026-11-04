#!/usr/bin/env bash
set -euo pipefail
cd /home/cbgpu/CBGPU-live/backend
umask 077
docker run --rm --network host \
  -v "$PWD/private:/private" \
  -e PGPASSFILE=/private/source.pgpass -e PGCONNECT_TIMEOUT=15 -e PGSSLMODE=require \
  --entrypoint pg_dump supabase/postgres:17.6.1.136 \
  -h aws-1-ap-northeast-2.pooler.supabase.com -p 5432 \
  -U cbgpu_export_20261028.fclbzjlcepoaifvcoxdw -d postgres \
  --format=custom --no-owner \
  --schema=public --schema=auth --schema=storage \
  --file=/private/source-full.dump
