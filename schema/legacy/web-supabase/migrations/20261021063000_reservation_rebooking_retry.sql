-- Retry for installations where the original rebooking migration was run from
-- the SQL editor before its column-name type cast fix was deployed.
--
-- This intentionally checks every required key separately instead of comparing
-- PostgreSQL name[] and text[] arrays.
do $$
declare
  item record;
begin
  for item in
    select
      idx.relname as index_name,
      con.conname as constraint_name
    from pg_index ind
    join pg_class idx on idx.oid = ind.indexrelid
    join pg_class tbl on tbl.oid = ind.indrelid
    join pg_namespace ns on ns.oid = tbl.relnamespace
    left join pg_constraint con on con.conindid = ind.indexrelid
    where ns.nspname = 'public'
      and tbl.relname = 'reservations'
      and ind.indisunique
      and not ind.indisprimary
      and ind.indpred is null
      and exists (
        select 1
        from unnest(ind.indkey) as key(attnum)
        join pg_attribute att
          on att.attrelid = ind.indrelid
         and att.attnum = key.attnum
        where att.attname::text = 'gpu_id'
      )
      and exists (
        select 1
        from unnest(ind.indkey) as key(attnum)
        join pg_attribute att
          on att.attrelid = ind.indrelid
         and att.attnum = key.attnum
        where att.attname::text = 'start_time'
      )
      and exists (
        select 1
        from unnest(ind.indkey) as key(attnum)
        join pg_attribute att
          on att.attrelid = ind.indrelid
         and att.attnum = key.attnum
        where att.attname::text = 'end_time'
      )
  loop
    if item.constraint_name is not null then
      execute format(
        'alter table public.reservations drop constraint %I',
        item.constraint_name
      );
    else
      execute format('drop index public.%I', item.index_name);
    end if;
  end loop;
end $$;

create index if not exists reservations_approved_availability_idx
  on public.reservations (gpu_id, end_time, start_time)
  where status = 'approved';
