-- The server currently names CPU containers and volumes with the first eight
-- UUID characters. Prevent one account from choosing a UUID that resolves to
-- another account's container, including when the new row is marked deleted.
-- Inspect and resolve any existing collisions before applying this migration.
begin;

do $$
begin
  if exists (
    select 1 from public.cpu_projects
    group by left(id::text, 8)
    having count(*) > 1
  ) then
    raise exception 'CPU project ID prefix collision exists; review rows before applying the unique index';
  end if;
end;
$$;

create unique index if not exists cpu_projects_container_prefix_unique
  on public.cpu_projects ((left(id::text, 8)));

commit;
