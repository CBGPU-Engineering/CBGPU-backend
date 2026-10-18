begin;

alter table public.contests add column logo_path text check (char_length(logo_path) <= 200);
create index contests_logo_path_idx on public.contests(logo_path) where logo_path is not null;
grant update(logo_path) on public.contests to authenticated;

-- Logos are deliberately public presentation assets, separate from private submissions.
insert into storage.buckets(id, name, public, file_size_limit, allowed_mime_types)
values ('contest-logos', 'contest-logos', true, 2097152, array['image/png','image/jpeg','image/webp']);

create policy contest_logos_read on storage.objects for select to authenticated
  using (bucket_id = 'contest-logos');
create policy contest_logos_upload on storage.objects for insert to authenticated with check (
  bucket_id = 'contest-logos'
  and split_part(name, '/', 1) = (select auth.uid())::text
  and name ~ '^[0-9a-f-]{36}/[0-9a-f-]{36}\.(png|jpg|webp)$'
);
-- Uploaded logos cannot be overwritten; deleting a logo still used by a contest is forbidden.
create policy contest_logos_cleanup on storage.objects for delete to authenticated using (
  bucket_id = 'contest-logos' and split_part(name, '/', 1) = (select auth.uid())::text
  and not exists (select 1 from public.contests c where c.logo_path = name)
);

create function public.validate_contest_logo() returns trigger
language plpgsql security invoker set search_path = '' as $$
begin
  if new.logo_path is null then return new; end if;
  if split_part(new.logo_path, '/', 1) <> new.host_id::text
    or new.logo_path !~ '^[0-9a-f-]{36}/[0-9a-f-]{36}\.(png|jpg|webp)$' then
    raise exception '본인이 업로드한 로고를 선택해 주세요.';
  end if;
  if not exists (select 1 from storage.objects o
    where o.bucket_id = 'contest-logos' and o.name = new.logo_path
      and o.metadata->>'mimetype' in ('image/png', 'image/jpeg', 'image/webp')
      and (o.metadata->>'size')::numeric between 1 and 2097152) then
    raise exception '로고는 업로드한 PNG, JPG, WebP 이미지(2MB 이하)만 사용할 수 있어요.';
  end if;
  return new;
end;
$$;
revoke all on function public.validate_contest_logo() from public, anon, authenticated;
create trigger validate_contest_logo before insert or update of logo_path on public.contests
  for each row execute function public.validate_contest_logo();

commit;
