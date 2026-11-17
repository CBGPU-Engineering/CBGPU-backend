begin;
-- Source-only additive capability: no object/bucket/publication visibility changes.
do $$begin
 if not exists(select 1 from pg_roles where rolname='cbgpu_public_asset_reader') then
  create role cbgpu_public_asset_reader nologin noinherit nosuperuser nobypassrls;
 elsif exists(select 1 from pg_roles where rolname='cbgpu_public_asset_reader' and(rolcanlogin or rolsuper or rolbypassrls or rolinherit)) then
  raise exception 'UNSAFE_PUBLIC_ASSET_ROLE';
 end if;
 execute format('grant connect on database %I to cbgpu_public_asset_reader',current_database());
end$$;
grant usage on schema cbgpu_schools to cbgpu_public_asset_reader;
-- This function is deliberately PRIVATE, not an anonymous PostgREST RPC. The
-- native service receives a minimal descriptor only after fresh publication,
-- school/module and object attribution checks. Browser DTOs receive no keys.
create or replace function cbgpu_schools.public_contest_banner(contest uuid)
returns jsonb language sql stable security definer set search_path='' as $$
 select jsonb_build_object('bucket','contest-presentation','object_key',o.name,
   'mime',o.metadata->>'mimetype','size',(o.metadata->>'size')::bigint)
 from public.contests c join storage.objects o on o.bucket_id='contest-presentation' and o.name=c.banner_path
 join storage.buckets b on b.id=o.bucket_id and b.public
 where c.id=contest and c.access_scope='PUBLIC' and c.lifecycle_status='published'
 and c.school_id is not null and cbgpu_schools.feature_enabled(c.school_id,'contests')
 and c.banner_path ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\.(png|jpg|webp)$'
 and split_part(o.name,'/',1)=c.host_id::text
 and o.metadata->>'mimetype' in('image/png','image/jpeg','image/webp')
 and case when o.metadata->>'size' ~ '^[0-9]{1,8}$' then (o.metadata->>'size')::bigint between 1 and 10485760 else false end
 limit 1
$$;
revoke all on function cbgpu_schools.public_contest_banner(uuid) from public,anon,authenticated,service_role;
grant execute on function cbgpu_schools.public_contest_banner(uuid) to cbgpu_public_asset_reader;
commit;
