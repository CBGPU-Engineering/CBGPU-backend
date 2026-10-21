-- A reservation that was cancelled or rejected must not reserve its old slot.
--
-- Older installations may have a UNIQUE constraint/index covering
-- (gpu_id, start_time, end_time).  That constraint also covers inactive rows,
-- so retrying the exact same slot after a cancellation fails before the
-- application can run its approved-reservation overlap check.
--
-- Remove only those legacy exact-slot unique constraints/indexes. Active
-- reservation conflicts remain protected by the existing application checks:
-- only `approved` rows block a slot, and the admin approval screen checks the
-- interval again before changing a request to approved.
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
      and (
        select array_agg(att.attname::text order by key.ordinality)
        from unnest(ind.indkey) with ordinality as key(attnum, ordinality)
        join pg_attribute att
          on att.attrelid = ind.indrelid
         and att.attnum = key.attnum
      ) @> array['gpu_id', 'start_time', 'end_time']::text[]
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

-- Matches the availability queries used by the reservation UI while excluding
-- cancelled/rejected history from the hot index.
create index if not exists reservations_approved_availability_idx
  on public.reservations (gpu_id, end_time, start_time)
  where status = 'approved';
