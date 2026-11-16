-- ============================================================
-- CBGPU Manufacturing (3D printing / laser cutting) migration
-- Run this in the Supabase SQL Editor
-- ============================================================

-- ── profiles extension: laser manager flag ─────────────────────────────
-- 3D printers can have a different manager per machine, so instead of a single
-- flag on profiles we use the printer_managers mapping table below (pick a
-- printer first, then assign the people who can operate it — set up in the
-- Manufacture tab of admin/page.tsx).
alter table profiles add column if not exists is_laser_cut_manager boolean default false;
alter table profiles drop column if exists is_3d_print_manager;

-- Wrapped in a security definer function (same pattern as is_admin()) to avoid
-- "infinite recursion detected in policy for relation profiles" when a policy
-- on profiles itself needs to select profiles (see supabase_security_hardening_migration.sql).
-- Note: being an admin (is_admin) does NOT automatically make someone a manufacturing
-- manager — the two roles are fully separate and must be granted explicitly from
-- the Manufacture tab in admin/page.tsx.
create or replace function public.is_laser_manager(uid uuid)
returns boolean
language sql
stable
security definer
set search_path = public
as $$
  select coalesce((select p.is_laser_cut_manager from public.profiles p where p.id = uid), false);
$$;

-- ── printers: 4x Bambu Lab X1C ──────────────────────────────────────────
-- 'done' = the print finished but is still sitting on the bed, waiting for the
-- manager to physically collect it. The printer only goes back to 'idle' once
-- collected — it isn't really free until the part is off the bed.
create table if not exists printers (
  id int primary key,
  name text not null,
  status text not null default 'idle' check (status in ('idle', 'printing', 'done', 'maintenance')),
  current_job_id uuid,
  started_at timestamptz,
  estimated_end_at timestamptz
);

insert into printers (id, name) values
  (1, '프린터 1'), (2, '프린터 2'), (3, '프린터 3'), (4, '프린터 4')
on conflict (id) do nothing;

-- ── printer_managers: per-printer manager mapping (many-to-many) ────────
create table if not exists printer_managers (
  printer_id int not null references printers(id) on delete cascade,
  user_id uuid not null references profiles(id) on delete cascade,
  created_at timestamptz default now(),
  primary key (printer_id, user_id)
);

-- Is this user the manager of this specific printer? (used to gate machine status changes)
create or replace function public.can_manage_printer(uid uuid, pid int)
returns boolean
language sql
stable
security definer
set search_path = public
as $$
  select exists(select 1 from public.printer_managers where user_id = uid and printer_id = pid);
$$;

-- Does this user manage at least one printer? (used for accept/reject rights and
-- whether the "3D manager" tab shows up at all)
create or replace function public.is_3d_manager(uid uuid)
returns boolean
language sql
stable
security definer
set search_path = public
as $$
  select exists(select 1 from public.printer_managers where user_id = uid);
$$;

-- ── print_jobs: 3D printing requests ─────────────────────────────────────
create table if not exists print_jobs (
  id uuid primary key default gen_random_uuid(),
  created_at timestamptz default now(),
  user_id uuid references auth.users(id) not null,
  user_email text,
  mode text not null check (mode in ('beginner', 'advanced')),
  manager_id uuid references profiles(id),
  preferred_printer_id int references printers(id),
  purpose text not null default '',
  quantity int not null default 1,
  plate_count int not null default 1,
  plates_completed int not null default 0,
  material text,
  color text,
  file_path text,
  file_name text,
  notes text default '',
  status text not null default 'pending'
    check (status in ('pending', 'accepted', 'queued', 'printing', 'done', 'rejected', 'canceled')),
  reject_reason text,
  printer_id int references printers(id),
  started_at timestamptz,
  estimated_end_at timestamptz,
  completed_at timestamptz,
  collected_at timestamptz,
  delivered_at timestamptz
);

do $$
begin
  if not exists (
    select 1 from pg_constraint where conname = 'printers_current_job_fk'
  ) then
    alter table printers
      add constraint printers_current_job_fk
      foreign key (current_job_id) references print_jobs(id);
  end if;
end $$;

-- ── print_materials: catalog of available filament (material, color) combos ──
-- Mirrors laser_materials in structure, but — unlike laser sheet stock, which is
-- intentionally NOT counted — filament spools ARE worth tracking a stock count for,
-- since managers restock/deplete individual spools over time.
create table if not exists print_materials (
  id uuid primary key default gen_random_uuid(),
  material text not null,
  color text not null,
  stock_count int not null default 0,
  created_at timestamptz default now(),
  unique (material, color)
);
alter table print_materials enable row level security;

drop policy if exists "print_materials_select" on print_materials;
create policy "print_materials_select" on print_materials for select
  using (auth.uid() is not null);

drop policy if exists "print_materials_manager_write" on print_materials;
create policy "print_materials_manager_write" on print_materials for all
  using (public.is_3d_manager(auth.uid()));

-- ── laser_jobs: laser cutting requests (no stock tracking, just type/thickness) ──
create table if not exists laser_jobs (
  id uuid primary key default gen_random_uuid(),
  created_at timestamptz default now(),
  user_id uuid references auth.users(id) not null,
  user_email text,
  manager_id uuid references profiles(id),
  purpose text not null default '',
  file_path text,
  file_name text,
  material text not null check (material in ('mdf', 'acrylic')),
  thickness_mm numeric,
  quantity int default 1,
  notes text default '',
  status text not null default 'pending'
    check (status in ('pending', 'accepted', 'in_progress', 'done', 'rejected', 'canceled')),
  reject_reason text,
  completed_at timestamptz,
  delivered_at timestamptz
);

-- ── laser_materials: catalog of requestable (material, thickness) combos ──
-- We don't track stock counts — just "what we currently carry" — so requesters
-- can only pick a combination that's registered here (no free-typed thickness).
create table if not exists laser_materials (
  id uuid primary key default gen_random_uuid(),
  material text not null check (material in ('mdf', 'acrylic')),
  thickness_mm numeric not null,
  created_at timestamptz default now(),
  unique (material, thickness_mm)
);
alter table laser_materials enable row level security;

drop policy if exists "laser_materials_select" on laser_materials;
create policy "laser_materials_select" on laser_materials for select
  using (auth.uid() is not null);

drop policy if exists "laser_materials_manager_write" on laser_materials;
create policy "laser_materials_manager_write" on laser_materials for all
  using (public.is_laser_manager(auth.uid()));

-- Safety net in case this migration was already run once (before these columns existed)
alter table print_jobs add column if not exists purpose text not null default '';
alter table laser_jobs add column if not exists purpose text not null default '';
alter table print_jobs add column if not exists quantity int not null default 1;
alter table print_jobs add column if not exists material text;
alter table print_jobs add column if not exists color text;
alter table print_jobs add column if not exists reject_reason text;
alter table laser_jobs add column if not exists reject_reason text;
alter table print_jobs add column if not exists collected_at timestamptz;
alter table print_jobs add column if not exists preferred_printer_id int references printers(id);
alter table print_jobs add column if not exists plate_count int not null default 1;
alter table print_jobs add column if not exists plates_completed int not null default 0;
alter table print_jobs add column if not exists delivered_at timestamptz;
alter table laser_jobs add column if not exists delivered_at timestamptz;
alter table print_materials add column if not exists stock_count int not null default 0;

-- Add the 'done' printer status if this migration ran before it existed.
alter table printers drop constraint if exists printers_status_check;
alter table printers add constraint printers_status_check check (status in ('idle', 'printing', 'done', 'maintenance'));

-- Clean up objects from an earlier "embedded slicer workspace" design.
-- We dropped that approach — there's no spare server to keep a slicer container
-- running at all times, so advanced-mode users slice locally in their own
-- Bambu Studio and upload the already-sliced file directly.
alter table print_jobs drop column if exists sliced_file_path;
alter table print_jobs drop column if exists sliced_file_name;
drop table if exists manufacturing_settings;

-- ── storage: private bucket for uploaded design files ────────────────────
insert into storage.buckets (id, name, public)
values ('manufacturing', 'manufacturing', false)
on conflict (id) do nothing;

update storage.buckets set file_size_limit = 209715200 where id = 'manufacturing'; -- 200MB

-- ── RLS ───────────────────────────────────────────────────────────────
alter table printers         enable row level security;
alter table printer_managers enable row level security;
alter table print_jobs       enable row level security;
alter table laser_jobs       enable row level security;

-- printers: anyone can read status, only that printer's manager can change it
drop policy if exists "printers_select_all" on printers;
create policy "printers_select_all" on printers for select using (auth.uid() is not null);

drop policy if exists "printers_manager_write" on printers;
create policy "printers_manager_write" on printers for update
  using (public.can_manage_printer(auth.uid(), id));

-- printer_managers: anyone can read assignments (to show who manages what), only admins can change them
drop policy if exists "printer_managers_select" on printer_managers;
create policy "printer_managers_select" on printer_managers for select
  using (auth.uid() is not null);

drop policy if exists "printer_managers_admin_write" on printer_managers;
create policy "printer_managers_admin_write" on printer_managers for all
  using (public.is_admin(auth.uid()));

-- print_jobs: owner, the assigned manager, any 3D manager, or an admin
drop policy if exists "print_jobs_select" on print_jobs;
create policy "print_jobs_select" on print_jobs for select
  using (
    user_id = auth.uid()
    or manager_id = auth.uid()
    or public.is_3d_manager(auth.uid())
  );

drop policy if exists "print_jobs_insert" on print_jobs;
create policy "print_jobs_insert" on print_jobs for insert
  with check (user_id = auth.uid());

drop policy if exists "print_jobs_update" on print_jobs;
create policy "print_jobs_update" on print_jobs for update
  using (
    user_id = auth.uid()
    or public.is_3d_manager(auth.uid())
  );

-- laser_jobs: owner, the assigned manager, any laser manager, or an admin
drop policy if exists "laser_jobs_select" on laser_jobs;
create policy "laser_jobs_select" on laser_jobs for select
  using (
    user_id = auth.uid()
    or manager_id = auth.uid()
    or public.is_laser_manager(auth.uid())
  );

drop policy if exists "laser_jobs_insert" on laser_jobs;
create policy "laser_jobs_insert" on laser_jobs for insert
  with check (user_id = auth.uid());

drop policy if exists "laser_jobs_update" on laser_jobs;
create policy "laser_jobs_update" on laser_jobs for update
  using (
    user_id = auth.uid()
    or public.is_laser_manager(auth.uid())
  );

-- storage.objects (manufacturing bucket): the first path segment is the uploader's
-- user_id. Owners can read their own folder; everyone else needs to be a 3D/laser
-- manager.
drop policy if exists "manufacturing_owner_insert" on storage.objects;
create policy "manufacturing_owner_insert" on storage.objects for insert
  with check (bucket_id = 'manufacturing' and (storage.foldername(name))[1] = auth.uid()::text);

drop policy if exists "manufacturing_owner_select" on storage.objects;
create policy "manufacturing_owner_select" on storage.objects for select
  using (
    bucket_id = 'manufacturing'
    and (
      (storage.foldername(name))[1] = auth.uid()::text
      or public.is_3d_manager(auth.uid())
      or public.is_laser_manager(auth.uid())
    )
  );

drop policy if exists "manufacturing_owner_delete" on storage.objects;
create policy "manufacturing_owner_delete" on storage.objects for delete
  using (bucket_id = 'manufacturing' and (storage.foldername(name))[1] = auth.uid()::text);

-- ── queue length counters (for regular requesters) ───────────────────────
-- A requester can only select their own rows, so they have no way to see how
-- backed up the overall queue is. These functions return just a count, without
-- exposing anyone else's filename/email.
create or replace function public.print_queue_length()
returns int
language sql
stable
security definer
set search_path = public
as $$
  select count(*)::int from public.print_jobs where status in ('pending', 'accepted');
$$;
grant execute on function public.print_queue_length() to authenticated;

-- Per-printer plate backlog: how many plates are still waiting for each preferred printer.
-- A job's plates print one at a time, so while it's 'printing' only the plates after the one
-- currently on the bed count as backlog (plate_count - plates_completed - 1, floored at 0) —
-- the one actively printing right now is not "backed up". Same "count only, no row exposure"
-- pattern as print_queue_length().
create or replace function public.printer_plate_backlog()
returns table(printer_id int, plates bigint)
language sql
stable
security definer
set search_path = public
as $$
  select
    preferred_printer_id,
    coalesce(sum(
      case
        when status in ('pending', 'accepted') then plate_count
        when status = 'printing' then greatest(plate_count - plates_completed - 1, 0)
        else 0
      end
    ), 0)
  from public.print_jobs
  where status in ('pending', 'accepted', 'printing') and preferred_printer_id is not null
  group by preferred_printer_id;
$$;
grant execute on function public.printer_plate_backlog() to authenticated;

create or replace function public.laser_queue_length()
returns int
language sql
stable
security definer
set search_path = public
as $$
  select count(*)::int from public.laser_jobs where status in ('pending', 'accepted');
$$;
grant execute on function public.laser_queue_length() to authenticated;
