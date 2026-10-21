-- Show booked time slots without exposing other users' reservation details.
begin;

create or replace function public.get_gpu_reservation_schedule(
  p_from timestamptz,
  p_to timestamptz,
  p_gpu_id integer default null
)
returns table (
  id uuid,
  gpu_id integer,
  start_time timestamptz,
  end_time timestamptz
)
language sql
stable
security definer
set search_path = ''
as $$
  select r.id, r.gpu_id, r.start_time, r.end_time
  from public.reservations r
  where r.status = 'approved'
    and r.start_time < p_to
    and r.end_time > p_from
    and (p_gpu_id is null or r.gpu_id = p_gpu_id)
  order by r.start_time;
$$;

revoke all on function public.get_gpu_reservation_schedule(timestamptz, timestamptz, integer) from public;
grant execute on function public.get_gpu_reservation_schedule(timestamptz, timestamptz, integer) to anon, authenticated;

-- Booking forms must also avoid already pending requests. The dashboard
-- calendar deliberately uses the approved-only function above.
create or replace function public.get_gpu_booking_schedule(
  p_from timestamptz,
  p_to timestamptz,
  p_gpu_id integer default null
)
returns table (
  id uuid,
  gpu_id integer,
  start_time timestamptz,
  end_time timestamptz
)
language sql
stable
security definer
set search_path = ''
as $$
  select r.id, r.gpu_id, r.start_time, r.end_time
  from public.reservations r
  where r.status in ('pending', 'approved')
    and r.start_time < p_to
    and r.end_time > p_from
    and (p_gpu_id is null or r.gpu_id = p_gpu_id)
  order by r.start_time;
$$;

revoke all on function public.get_gpu_booking_schedule(timestamptz, timestamptz, integer) from public;
grant execute on function public.get_gpu_booking_schedule(timestamptz, timestamptz, integer) to anon, authenticated;

commit;
