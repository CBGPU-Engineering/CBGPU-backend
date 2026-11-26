# Avatars: rollout, rollback, impact

## Order

1. **Backend migration 1** `20261126120000_default_avatars.sql` (applied to production on 2026-11-26).
2. **Web #77** (applied).
3. **Backend migration 2** `20261126150000_avatar_surfaces.sql`. Needs the owner of the `cbgpu_schools` functions
   (`supabase_admin`); `postgres` cannot rename them. The delivery agent does **not** apply `supabase/migrations/`
   (it treats migration-only commits as "no service change"), so this is a manual step.
4. **Web** PR "cross-school avatars + avatar proxy" (works before step 3, just without pictures for hidden people).
5. Verify the proxy on production: `GET https://cbgpu.com/api/avatars/9.x/thumbs/png?seed=<uuid>&size=256` returns `200 image/png`,
   `x-avatar-cache: miss` then `hit`.
6. **Switch to the own domain** (no client change):
   `update public.avatar_settings set base_url = 'https://cbgpu.com/api/avatars';`
   Every `avatarUrl` now points at our domain. Browsers and apps no longer contact the provider.
7. Web PR "native app writes" (separate, security review).

Never do step 6 before step 5.

## Apply migration 2 (production)

```sh
set -e
{ echo "set lock_timeout='5s';"; cat supabase/migrations/20261126150000_avatar_surfaces.sql; } \
  | docker exec -i cbgpu-supabase-db psql -X -v ON_ERROR_STOP=1 -U supabase_admin -d postgres
docker exec cbgpu-supabase-db psql -U postgres -c "notify pgrst, 'reload schema'"
```

The file is one transaction, takes brief locks, and re-running it is a no-op.
Check afterwards: wrappers present, owner-only execute rights, 181 profiles unchanged, `avatar_url` check validated.

## Rollback

| Step | Undo |
| --- | --- |
| 6 own-domain URLs | `update public.avatar_settings set base_url = 'https://api.dicebear.com';` (immediate, no deploy) |
| 4 / 7 web | revert the merge; the delivery agent redeploys the previous release |
| 3 migration 2 | run the block below as `supabase_admin`, in one transaction |
| 1 migration 1 | only if needed: drop view/functions/trigger, then `alter table public.profiles drop column avatar_seed` (this loses the seeds and therefore every account's default avatar) |

```sql
begin;
-- wrappers back to the originals
do $$ declare s record; begin
 for s in select * from (values ('message_json','cbgpu_schools.messages'),('dm_message_json','cbgpu_schools.direct_messages'),
   ('member_json','cbgpu_schools.memberships'),('operator_nomination','cbgpu_schools.contest_operator_nominations')) t(fn,argtype) loop
  execute format('drop function cbgpu_schools.%I(%s)',s.fn,s.argtype);
  execute format('alter function cbgpu_schools.%I(%s) rename to %I',s.fn||'_core',s.argtype,s.fn);
 end loop;
 drop function cbgpu_schools.gpu_admin_booking_item(text,jsonb);
 alter function cbgpu_schools.gpu_admin_booking_item_core(text,jsonb) rename to gpu_admin_booking_item;
end $$;
drop function cbgpu_schools.avatar_fields(uuid);
drop function public.cbgpu_avatar_urls(uuid[]);
alter table public.profiles drop constraint profiles_avatar_url_storage_chk;
commit;
```

## Impact

| Area | Effect |
| --- | --- |
| `public.profiles` | + `avatar_seed` (unique, immutable), + check on `avatar_url`. No existing column changes |
| Community / DM / members / nominations / GPU admin responses | + `avatarUrl`, + `profileImageUrl`. Every existing key is unchanged (verified key by key against the production schema) |
| Permissions | wrappers and helper are owner-only (same as the originals); `cbgpu_avatar_urls` is `authenticated` + `service_role`, needs a signed-in session; `anon` has nothing new |
| Locks | `ALTER TABLE profiles` (migration 1, done) and `ALTER FUNCTION … RENAME` (migration 2): milliseconds; `lock_timeout` 5 s protects live traffic |
| Writers | a client can no longer store an arbitrary URL in `avatar_url`; the web and app upload to `avatars/<own id>/avatar.jpg`, which passes |
| Provider traffic | after step 6, the provider only ever sees our server |

## Data check before the check constraint (2026-11-26)

5 of 181 profiles have a photo. All 5 are `https://cbgpu.com/backend/storage/v1/object/public/avatars/<own id>/avatar.(jpg|jpeg)?v=<ts>`.
0 blank values, 0 foreign hosts, 0 rows owned by someone else's path. The constraint accepts all 5.
