-- Protect GPU reservations even when a client has stale or RLS-filtered data.
-- Existing pending/approved conflicts are intentionally left for admin review.
-- Run once in the Supabase SQL Editor or via the Management API.
begin;

create extension if not exists btree_gist;

alter table public.reservations
  add constraint reservations_valid_window
  check (
    gpu_id is not null
    and start_time is not null
    and end_time is not null
    and end_time > start_time
    and status in ('pending', 'approved', 'rejected')
  );

-- The database, rather than the admin browser, guarantees that two approved
-- intervals cannot occupy one GPU. Half-open ranges allow adjacent bookings.
alter table public.reservations
  add constraint reservations_approved_no_overlap
  exclude using gist (
    gpu_id with =,
    tstzrange(start_time, end_time, '[)') with &&
  ) where (status = 'approved');

-- A permissive legacy INSERT policy let any authenticated user choose another
-- user_id or even status='approved'. Replace it with one owner/pending policy.
drop policy if exists "Enable insert for authenticated users only" on public.reservations;
drop policy if exists "Individuals can create their own reservations" on public.reservations;
drop policy if exists "Users insert" on public.reservations;
create policy reservations_insert_own_pending on public.reservations
  for insert to authenticated
  with check (user_id = auth.uid() and status = 'pending');

-- The old UI updates end_time directly. A narrow UPDATE policy makes that
-- operation work, while the trigger below forbids changes to every other field.
drop policy if exists reservations_owner_stop on public.reservations;
create policy reservations_owner_stop on public.reservations
  for update to authenticated
  using (user_id = auth.uid() and status = 'approved')
  with check (user_id = auth.uid() and status = 'approved');

create or replace function public.guard_gpu_reservation_write()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_now timestamptz := clock_timestamp();
  v_check_overlap boolean;
begin
  if new.gpu_id is null or new.start_time is null or new.end_time is null
     or new.status not in ('pending', 'approved', 'rejected') then
    raise exception 'GPU 예약 필수 값이 올바르지 않습니다.' using errcode = '23514';
  end if;

  -- Direct SQL and trusted server calls retain administrative capability.
  if auth.role() = 'authenticated' and not public.is_admin(auth.uid()) then
    if tg_op = 'INSERT' then
      if new.user_id is distinct from auth.uid() or new.status <> 'pending' then
        raise exception '본인 명의의 대기 예약만 신청할 수 있습니다.' using errcode = '42501';
      end if;
      -- Weekly clients may send Monday midnight after the week has begun.
      -- Reserve only the remaining time; the server clock is authoritative.
      if new.start_time < v_now then
        new.start_time := v_now + interval '2 minutes';
      end if;
    elsif tg_op = 'UPDATE' then
      if old.user_id is distinct from auth.uid()
         or old.status <> 'approved'
         or old.start_time > v_now or old.end_time <= v_now
         or row(new.id, new.user_id, new.user_email, new.gpu_id, new.status,
                new.created_at, new.start_time, new.ssh_password)
            is distinct from
            row(old.id, old.user_id, old.user_email, old.gpu_id, old.status,
                old.created_at, old.start_time, old.ssh_password) then
        raise exception '진행 중인 본인 예약의 사용 중단만 가능합니다.' using errcode = '42501';
      end if;
      new.end_time := v_now;
    end if;
  end if;

  if new.end_time <= new.start_time then
    raise exception '예약 종료 시각은 시작 시각보다 늦어야 합니다.' using errcode = '23514';
  end if;

  -- Shortening an existing reservation cannot create a new overlap. Let the
  -- owner stop even if an older conflicting pending request already exists.
  v_check_overlap := tg_op = 'INSERT';
  if tg_op = 'UPDATE' then
    v_check_overlap := new.status is distinct from old.status
      or new.gpu_id is distinct from old.gpu_id
      or new.start_time < old.start_time
      or new.end_time > old.end_time;
  end if;

  if v_check_overlap and new.status in ('pending', 'approved') then
    -- Serialize writers for the same GPU so two simultaneous requests cannot
    -- both pass a read-before-write conflict check.
    perform pg_catalog.pg_advisory_xact_lock(91370, new.gpu_id);
    if exists (
      select 1 from public.reservations r
      where r.gpu_id = new.gpu_id
        and r.id <> new.id
        and r.status in ('pending', 'approved')
        and r.start_time < new.end_time
        and r.end_time > new.start_time
    ) then
      raise exception '해당 시간에 이미 대기 또는 확정된 GPU 예약이 있습니다.'
        using errcode = '23P01';
    end if;
  end if;
  return new;
end;
$$;

drop trigger if exists guard_gpu_reservation_write on public.reservations;
create trigger guard_gpu_reservation_write
  before insert or update on public.reservations
  for each row execute function public.guard_gpu_reservation_write();

commit;
