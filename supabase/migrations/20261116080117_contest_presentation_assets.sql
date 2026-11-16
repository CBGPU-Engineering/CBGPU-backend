begin;
alter table public.contests add column banner_path text check(char_length(banner_path)<=200);
create index contests_banner_path_idx on public.contests(banner_path) where banner_path is not null;
grant update(description,banner_path) on public.contests to authenticated;
-- Presentation editing does not permit changing ownership or submission fields.
CREATE OR REPLACE FUNCTION public.validate_contest()
 RETURNS trigger
 LANGUAGE plpgsql
 SET search_path TO ''
AS $function$
declare f jsonb; seen text[] := '{}';
begin
  if tg_op = 'UPDATE' then
    if new.id <> old.id or new.host_id <> old.host_id or new.fields <> old.fields
      or new.title <> old.title  or new.created_at <> old.created_at then
      raise exception 'Contest fields and ownership are immutable';
    end if;
  end if;
  if tg_op = 'INSERT' or new.ends_at is distinct from old.ends_at then
    if new.ends_at is not null and (not isfinite(new.ends_at) or new.ends_at <= clock_timestamp()) then
      raise exception '마감 시간은 현재 서버 시각 이후로 지정해 주세요.';
    end if;
  end if;
  if tg_op = 'UPDATE' then return new; end if;
  if jsonb_typeof(new.fields) <> 'array' or jsonb_array_length(new.fields) not between 1 and 30 then
    raise exception '제출 항목은 1~30개로 구성해 주세요.';
  end if;
  for f in select value from jsonb_array_elements(new.fields) loop
    if jsonb_typeof(f) <> 'object'
      or coalesce(f->>'id', '') !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
      or f->>'id' = any(seen)
      or coalesce(f->>'type', '') not in ('file', 'link', 'text')
      or coalesce(jsonb_typeof(f->'label'), '') <> 'string'
      or char_length(btrim(coalesce(f->>'label', ''))) not between 1 and 200
      or coalesce(jsonb_typeof(f->'description'), '') <> 'string'
      or char_length(f->>'description') > 1000
      or coalesce(jsonb_typeof(f->'required'), '') <> 'boolean' then
      raise exception '제출 항목 설정을 확인해 주세요.';
    end if;
    seen := array_append(seen, f->>'id');
    if f->>'type' = 'file' then
      if coalesce(jsonb_typeof(f->'extensions'), '') <> 'string'
        or char_length(f->>'extensions') > 200
        or ((f->>'extensions') <> '' and (f->>'extensions') !~ '^\.[a-z0-9]+(\.[a-z0-9]+)*(,\.[a-z0-9]+(\.[a-z0-9]+)*)*$')
        or coalesce(f->>'max_size_mb', '') !~ '^[0-9]+$' then
        raise exception '파일 확장자와 용량 제한을 확인해 주세요.';
      end if;
      if (f->>'max_size_mb')::numeric not between 1 and 50 then raise exception '파일 용량은 1~50MB로 지정해 주세요.'; end if;
    end if;
  end loop;
  return new;
end;
$function$;



insert into storage.buckets(id,name,public,file_size_limit,allowed_mime_types)
values ('contest-presentation','contest-presentation',true,10485760,array['image/png','image/jpeg','image/webp']);
create policy contest_presentation_read on storage.objects for select to authenticated using(bucket_id='contest-presentation');
create policy contest_presentation_upload on storage.objects for insert to authenticated with check(
 bucket_id='contest-presentation' and split_part(name,'/',1)=(select auth.uid())::text
 and name ~ '^[0-9a-f-]{36}/[0-9a-f-]{36}\.(png|jpg|webp)$'
);
create policy contest_presentation_cleanup on storage.objects for delete to authenticated using(
 bucket_id='contest-presentation' and split_part(name,'/',1)=(select auth.uid())::text
 and not exists(select 1 from public.contests c where c.banner_path=name or strpos(c.description,name)>0)
);
create function public.validate_contest_banner() returns trigger language plpgsql security invoker set search_path='' as $$
begin
 if new.banner_path is null then return new; end if;
 if tg_op='UPDATE' and new.banner_path is not distinct from old.banner_path then return new; end if;
 if new.banner_path !~ '^[0-9a-f-]{36}/[0-9a-f-]{36}\.(png|jpg|webp)$' or not exists(
  select 1 from storage.objects o where o.bucket_id='contest-presentation' and o.name=new.banner_path
  and o.metadata->>'mimetype' in ('image/png','image/jpeg','image/webp')
  and case when o.metadata->>'size' ~ '^[0-9]+$' then (o.metadata->>'size')::numeric between 1 and 10485760 else false end
 ) then raise exception '업로드한 PNG, JPG, WebP 배너 이미지를 선택해 주세요.'; end if;
 return new;
end;$$;
revoke all on function public.validate_contest_banner() from public,anon,authenticated;
create trigger validate_contest_banner before insert or update of banner_path on public.contests for each row execute function public.validate_contest_banner();
notify pgrst,'reload schema';
commit;
