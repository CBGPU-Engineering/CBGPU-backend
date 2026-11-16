-- ============================================================
-- CBGPU 프로필 사진 마이그레이션
-- Supabase SQL Editor에서 실행하세요
-- ============================================================

alter table profiles add column if not exists avatar_url text;

-- 공개적으로 읽을 수 있는 'avatars' 버킷. 경로는 avatars/{user_id}/파일명 형식으로 쓴다.
insert into storage.buckets (id, name, public)
values ('avatars', 'avatars', true)
on conflict (id) do nothing;

create policy "avatar_public_read" on storage.objects for select
  using (bucket_id = 'avatars');

create policy "avatar_owner_insert" on storage.objects for insert
  with check (bucket_id = 'avatars' and (storage.foldername(name))[1] = auth.uid()::text);

create policy "avatar_owner_update" on storage.objects for update
  using (bucket_id = 'avatars' and (storage.foldername(name))[1] = auth.uid()::text);

create policy "avatar_owner_delete" on storage.objects for delete
  using (bucket_id = 'avatars' and (storage.foldername(name))[1] = auth.uid()::text);
