begin;
-- Preserve every historical value without inventing retroactive consent.
alter table public.contest_submissions add column if not exists student_id_consent boolean not null default false,add column if not exists student_id_consented_at timestamptz;
create or replace function cbgpu_schools.guard_submission_identity_consent() returns trigger language plpgsql security definer set search_path='' as $$declare explicit_required boolean;begin
 if auth.uid() is null then return new;end if;
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if tg_op='UPDATE' and auth.uid()<>old.user_id then
 if(new.student_id,new.student_id_consent) is distinct from(old.student_id,old.student_id_consent) then raise exception 'SUBMISSION_IDENTITY_CONSENT_OWNER_REQUIRED' using errcode='42501';end if;
 new.student_id_consented_at:=old.student_id_consented_at;return new;
 end if;
 if(tg_op='INSERT' or new.student_id is distinct from old.student_id) and length(coalesce(new.student_id,''))>80 then raise exception 'INVALID_STUDENT_ID_SNAPSHOT' using errcode='23514';end if;
 if coalesce(btrim(new.student_id),'')='' then new.student_id:='';new.student_id_consent:=false;new.student_id_consented_at:=null;return new;end if;
 select not(c.school_id='cbsh' and c.access_scope='SCHOOL') into explicit_required from public.contests c where c.id=new.contest_id;
 if coalesce(explicit_required,true) and not new.student_id_consent then raise exception 'STUDENT_ID_CONSENT_REQUIRED' using errcode='23514';end if;
 if new.student_id_consent then
 if tg_op='UPDATE' and old.student_id_consent and new.student_id is not distinct from old.student_id then new.student_id_consented_at:=coalesce(old.student_id_consented_at,clock_timestamp());else new.student_id_consented_at:=clock_timestamp();end if;
 else new.student_id_consented_at:=null;end if;return new;
end$$;
revoke all on function cbgpu_schools.guard_submission_identity_consent() from public,anon,authenticated;
drop trigger if exists aa_submission_identity_consent on public.contest_submissions;
create trigger aa_submission_identity_consent before insert or update on public.contest_submissions for each row execute function cbgpu_schools.guard_submission_identity_consent();
commit;
