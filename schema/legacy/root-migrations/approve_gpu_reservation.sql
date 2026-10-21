-- Approve and rotate the GPU password in one transaction. If the reservation
-- conflicts, the status and password both remain unchanged.
begin;

create or replace function public.approve_gpu_reservation(p_reservation_id uuid)
returns void
language plpgsql
security definer
set search_path = ''
as $$
declare
  v_gpu_id integer;
begin
  if auth.uid() is null or not public.is_admin(auth.uid()) then
    raise exception '관리자만 GPU 예약을 승인할 수 있습니다.' using errcode = '42501';
  end if;

  select r.gpu_id into v_gpu_id
  from public.reservations r
  where r.id = p_reservation_id and r.status = 'pending'
  for update;

  if not found then
    raise exception '승인 대기 예약을 찾지 못했습니다.' using errcode = 'P0002';
  end if;

  update public.reservations
  set status = 'approved'
  where id = p_reservation_id;

  insert into public.gpus (id, password)
  values (v_gpu_id, pg_catalog.encode(extensions.gen_random_bytes(16), 'hex'))
  on conflict (id) do update set password = excluded.password;
end;
$$;

revoke all on function public.approve_gpu_reservation(uuid) from public;
grant execute on function public.approve_gpu_reservation(uuid) to authenticated;

commit;
