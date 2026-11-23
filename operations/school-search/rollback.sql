-- Reviewed function-only rollback. No row, ID, draft or permission edits.
BEGIN;
DO $$DECLARE d text;old_branch text:=$old$if action='directory' then return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.summary(z)-'gpu'-'logo_path'),'[]') from(select * from cbgpu_schools.schools where status in('LISTED','UNCLAIMED','APPROVED') and (official_name ilike '%'||q||'%' or display_name ilike '%'||q||'%') order by case when id='cbsh' then 0 else 1 end,display_name limit lim offset off) z),'total',(select count(*) from cbgpu_schools.schools where status in('LISTED','UNCLAIMED','APPROVED') and(official_name ilike '%'||q||'%' or display_name ilike '%'||q||'%')));end if;$old$;new_branch text:=$new$if action='directory' then return public.cbgpu_public_school_directory(payload);end if;$new$;BEGIN
 d:=pg_get_functiondef('public.cbgpu_school_api(text,jsonb)'::regprocedure);
 IF position(new_branch IN d)>0 THEN EXECUTE replace(d,new_branch,old_branch);ELSIF position(old_branch IN d)=0 THEN RAISE EXCEPTION 'DIRECTORY_ROLLBACK_VERSION_MISMATCH';END IF;
END$$;
DO $$DECLARE d text;old_branch text:=$old$if s.status='APPROVED' then return jsonb_build_object('ok',false,'code','DUPLICATE_SCHOOL','existing_school_id',s.id);end if;$old$;new_branch text:=$new$if s.status='APPROVED' or cbgpu_schools.directory_registered_match(s) is not null then return jsonb_build_object('ok',false,'code','DUPLICATE_SCHOOL','existing_school_id',coalesce(cbgpu_schools.directory_registered_match(s),s.id));end if;$new$;BEGIN
 d:=pg_get_functiondef('public.cbgpu_school_catalog_application_api(text,jsonb)'::regprocedure);
 IF position(new_branch IN d)>0 THEN EXECUTE replace(d,new_branch,old_branch);ELSIF position(old_branch IN d)=0 THEN RAISE EXCEPTION 'DIRECTORY_ROLLBACK_VERSION_MISMATCH';END IF;
END$$;
DROP FUNCTION IF EXISTS public.cbgpu_public_school_directory(jsonb);
DROP FUNCTION IF EXISTS cbgpu_schools.canonical_directory_rows();
DROP FUNCTION IF EXISTS cbgpu_schools.directory_registered_match(cbgpu_schools.schools);
DROP FUNCTION IF EXISTS cbgpu_schools.directory_location(text,text);
DROP FUNCTION IF EXISTS cbgpu_schools.directory_province(text);
NOTIFY pgrst,'reload schema';
COMMIT;
