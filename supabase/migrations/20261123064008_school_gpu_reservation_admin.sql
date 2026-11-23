-- Source-only candidate. Explicit backup/approval required before production application.
-- No role, membership, RLS policy or table grants; existing platform approval RPC stays unchanged.
BEGIN;
CREATE OR REPLACE FUNCTION cbgpu_schools.gpu_admin_authorized(school text) RETURNS boolean
LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path='' AS $$
 SELECT auth.role() IS NOT DISTINCT FROM 'authenticated' AND public.cbgpu_personal_project_identity() AND cbgpu_accounts.current_session_valid()
 AND CASE WHEN jsonb_typeof(auth.jwt()->'exp')='number' AND auth.jwt()->>'exp'~'^[0-9]{1,12}$' THEN (auth.jwt()->>'exp')::bigint>extract(epoch FROM clock_timestamp()) ELSE false END
 AND cbgpu_schools.has_role(auth.uid(),school,ARRAY['SCHOOL_OWNER','SCHOOL_ADMIN','GPU_ADMIN'])
 AND EXISTS(SELECT FROM auth.sessions s WHERE s.user_id=auth.uid() AND s.id=CASE WHEN auth.jwt()->>'session_id'~'^[a-fA-F0-9-]{36}$' THEN(auth.jwt()->>'session_id')::uuid END AND(s.not_after IS NULL OR s.not_after>clock_timestamp()))
 AND EXISTS(SELECT FROM cbgpu_schools.memberships m WHERE m.user_id=auth.uid() AND m.school_id=school AND m.status='ACTIVE' AND m.verified AND m.starts_at<=clock_timestamp() AND(m.ends_at IS NULL OR m.ends_at>clock_timestamp()) AND m.school_roles&&ARRAY['SCHOOL_OWNER','SCHOOL_ADMIN','GPU_ADMIN'])
$$;
REVOKE ALL ON FUNCTION cbgpu_schools.gpu_admin_authorized(text) FROM PUBLIC,anon,authenticated,service_role;
CREATE OR REPLACE FUNCTION cbgpu_schools.gpu_admin_booking_rows(school text)
RETURNS TABLE(id uuid,source text,status text,start_time timestamptz,end_time timestamptz,created_at timestamptz,purpose text,gpu_id text,gpu_label text,owner_id uuid,full_name text,student_number text)
LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT b.id,'legacy',b.status,b.start_time,b.end_time,b.created_at,coalesce(to_jsonb(b)->>'purpose',''),b.gpu_id::text,'GPU #'||b.gpu_id,b.user_id,p.full_name,m.student_number
 FROM public.reservations b LEFT JOIN public.profiles p ON p.id=b.user_id LEFT JOIN cbgpu_schools.memberships m ON m.user_id=b.user_id AND m.school_id=school
 WHERE school='cbsh'
 UNION ALL
 SELECT b.id,'registry',b.status,b.starts_at,b.ends_at,b.created_at,'',r.id::text,r.name,b.owner_id,p.full_name,m.student_number
 FROM cbgpu_schools.gpu_resource_reservations b JOIN cbgpu_schools.gpu_resources r ON r.id=b.resource_id JOIN cbgpu_schools.gpu_nodes n ON n.id=r.node_id
 LEFT JOIN public.profiles p ON p.id=b.owner_id LEFT JOIN cbgpu_schools.memberships m ON m.user_id=b.owner_id AND m.school_id=school
 WHERE n.school_id=school
$$;
REVOKE ALL ON FUNCTION cbgpu_schools.gpu_admin_booking_rows(text) FROM PUBLIC,anon,authenticated,service_role;

CREATE OR REPLACE FUNCTION cbgpu_schools.gpu_admin_booking_item(school text,booking_row jsonb) RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT jsonb_build_object('id',b.id,'source',b.source,'status',b.status,'start_time',b.start_time,'end_time',b.end_time,'purpose',b.purpose,'gpu_id',b.gpu_id,'gpu_label',b.gpu_label,
 'owner',jsonb_build_object('id',b.owner_id,'full_name',b.full_name,'student_number',b.student_number),
 'allowed_actions',CASE
 WHEN b.source='legacy' AND b.status='pending' AND b.end_time>clock_timestamp() THEN
  CASE WHEN cbgpu_accounts.can_user_use_resource(b.owner_id,'gpu','cbsh') AND cbgpu_schools.gpu_notebook_permission(b.owner_id,'cbsh',greatest(b.start_time,clock_timestamp()),b.end_time)
   AND NOT EXISTS(SELECT FROM public.reservations a WHERE a.gpu_id::text=b.gpu_id AND a.id<>b.id AND a.status='approved' AND a.start_time<=clock_timestamp() AND a.end_time>clock_timestamp())
  THEN ' ["approve","reject"]'::jsonb ELSE '["reject"]'::jsonb END
 WHEN b.source='registry' AND b.status='RESERVED' AND b.end_time>clock_timestamp() THEN '["cancel","revoke"]'::jsonb ELSE '[]'::jsonb END)
 FROM jsonb_to_record(booking_row) AS b(id uuid,source text,status text,start_time timestamptz,end_time timestamptz,created_at timestamptz,purpose text,gpu_id text,gpu_label text,owner_id uuid,full_name text,student_number text)
$$;
REVOKE ALL ON FUNCTION cbgpu_schools.gpu_admin_booking_item(text,jsonb) FROM PUBLIC,anon,authenticated,service_role;

CREATE OR REPLACE FUNCTION public.school_gpu_reservation_admin(p_school_id text,p_action text,p_payload jsonb DEFAULT '{}') RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE uid uuid:=auth.uid(); claims text; item record; booking_id uuid; kind text; operation text; expected_end timestamptz; expected_status text; job_id uuid;
 lim int:=50; view text:='all'; query text:=''; gpu text:='all'; cursor_value jsonb; cursor_date timestamptz; cursor_id uuid; cursor_source text; batch jsonb; result jsonb; next_value text;
BEGIN
 IF cbgpu_schools.gpu_admin_authorized(p_school_id) IS NOT TRUE THEN RAISE EXCEPTION 'GPU_ADMIN_REQUIRED' USING ERRCODE='42501';END IF;
 IF jsonb_typeof(p_payload) IS DISTINCT FROM 'object' OR octet_length(p_payload::text)>4096 THEN RAISE EXCEPTION 'INVALID_REQUEST' USING ERRCODE='22023';END IF;
 PERFORM set_config('lock_timeout','1500ms',true);
 IF p_action='list' THEN
  IF EXISTS(SELECT FROM jsonb_object_keys(p_payload) k WHERE k NOT IN('limit','cursor','status','q','gpu')) THEN RAISE EXCEPTION 'INVALID_REQUEST' USING ERRCODE='22023';END IF;
  lim:=coalesce((p_payload->>'limit')::int,50);view:=coalesce(p_payload->>'status','all');query:=btrim(coalesce(p_payload->>'q',''));gpu:=coalesce(p_payload->>'gpu','all');
  IF lim NOT BETWEEN 1 AND 100 OR length(query)>200 OR length(gpu)>100 OR view NOT IN('all','pending','approved','rejected','reserved','cancelled','revoked','active','upcoming','ended') THEN RAISE EXCEPTION 'INVALID_REQUEST' USING ERRCODE='22023';END IF;
  IF p_payload->>'cursor' IS NOT NULL THEN
   IF length(p_payload->>'cursor')>512 THEN RAISE EXCEPTION 'INVALID_CURSOR' USING ERRCODE='22023';END IF;
   cursor_value:=(p_payload->>'cursor')::jsonb;
   IF jsonb_typeof(cursor_value) IS DISTINCT FROM 'object' OR EXISTS(SELECT FROM jsonb_object_keys(cursor_value) k WHERE k NOT IN('created_at','id','source')) THEN RAISE EXCEPTION 'INVALID_CURSOR' USING ERRCODE='22023';END IF;
   cursor_date:=(cursor_value->>'created_at')::timestamptz;cursor_id:=(cursor_value->>'id')::uuid;cursor_source:=cursor_value->>'source';
   IF cursor_id IS NULL OR cursor_source IS NULL OR cursor_source NOT IN('legacy','registry') OR(cursor_date IS NOT NULL AND NOT isfinite(cursor_date)) THEN RAISE EXCEPTION 'INVALID_CURSOR' USING ERRCODE='22023';END IF;
  END IF;
  SELECT coalesce(jsonb_agg(to_jsonb(x) ORDER BY x.created_at DESC NULLS LAST,x.id DESC,x.source DESC),'[]') INTO batch FROM(
   SELECT b.* FROM cbgpu_schools.gpu_admin_booking_rows(p_school_id) b
   WHERE(gpu='all' OR b.gpu_id=gpu)
   AND(view='all' OR lower(b.status)=view OR view='ended' AND b.end_time<=clock_timestamp()
    OR view='active' AND b.status IN('approved','RESERVED') AND b.start_time<=clock_timestamp() AND b.end_time>clock_timestamp()
    OR view='upcoming' AND b.status IN('approved','RESERVED') AND b.start_time>clock_timestamp() AND b.end_time>clock_timestamp())
   AND(query='' OR concat_ws(' ',b.full_name,b.student_number,b.purpose,b.gpu_label,b.id::text) ILIKE '%'||query||'%')
   AND(cursor_id IS NULL OR cursor_date IS NULL AND b.created_at IS NULL AND(b.id,b.source)<(cursor_id,cursor_source)
    OR cursor_date IS NOT NULL AND(b.created_at<cursor_date OR b.created_at IS NULL OR b.created_at=cursor_date AND(b.id,b.source)<(cursor_id,cursor_source)))
   ORDER BY b.created_at DESC NULLS LAST,b.id DESC,b.source DESC LIMIT lim+1
  ) x;
  SELECT coalesce(jsonb_agg(cbgpu_schools.gpu_admin_booking_item(p_school_id,v) ORDER BY ord),'[]') INTO result FROM jsonb_array_elements(batch) WITH ORDINALITY a(v,ord) WHERE ord<=lim;
  next_value:=NULL;
  IF jsonb_array_length(batch)>lim THEN next_value:=jsonb_build_object('id',batch->(lim-1)->'id','source',batch->(lim-1)->'source','created_at',batch->(lim-1)->'created_at')::text;END IF;
  RETURN jsonb_build_object('school_id',p_school_id,'items',result,'next_cursor',next_value,'server_now',clock_timestamp());
 ELSIF p_action='action' THEN
  IF EXISTS(SELECT FROM jsonb_object_keys(p_payload) k WHERE k NOT IN('reservation_id','source','action','expected_status','expected_end_time')) THEN RAISE EXCEPTION 'INVALID_REQUEST' USING ERRCODE='22023';END IF;
  booking_id:=(p_payload->>'reservation_id')::uuid;kind:=p_payload->>'source';operation:=p_payload->>'action';expected_status:=p_payload->>'expected_status';expected_end:=(p_payload->>'expected_end_time')::timestamptz;
  IF booking_id IS NULL OR kind IS NULL OR operation IS NULL OR expected_status IS NULL OR expected_end IS NULL OR NOT isfinite(expected_end) OR kind NOT IN('legacy','registry') OR operation NOT IN('approve','reject','cancel','revoke') THEN RAISE EXCEPTION 'INVALID_REQUEST' USING ERRCODE='22023';END IF;
  IF kind='legacy' THEN
   IF p_school_id<>'cbsh' OR operation NOT IN('approve','reject') THEN RAISE EXCEPTION 'RESERVATION_NOT_FOUND' USING ERRCODE='P0002';END IF;
   -- Also fences direct Supabase writers, approval RPCs and end-time extensions.
   LOCK TABLE public.reservations IN EXCLUSIVE MODE;
  ELSE
   IF operation NOT IN('cancel','revoke') THEN RAISE EXCEPTION 'INVALID_ACTION' USING ERRCODE='22023';END IF;
   LOCK TABLE cbgpu_schools.gpu_resource_reservations IN EXCLUSIVE MODE;
  END IF;
  IF cbgpu_schools.gpu_admin_authorized(p_school_id) IS NOT TRUE THEN RAISE EXCEPTION 'GPU_ADMIN_REQUIRED' USING ERRCODE='42501';END IF;
  SELECT * INTO item FROM cbgpu_schools.gpu_admin_booking_rows(p_school_id) b WHERE b.id=booking_id AND b.source=kind;
  IF NOT FOUND THEN RAISE EXCEPTION 'RESERVATION_NOT_FOUND' USING ERRCODE='P0002';END IF;
  IF item.status IS DISTINCT FROM expected_status OR item.end_time IS DISTINCT FROM expected_end THEN RAISE EXCEPTION 'RESERVATION_CHANGED' USING ERRCODE='40001';END IF;
  IF item.end_time<=clock_timestamp() THEN RAISE EXCEPTION 'RESERVATION_EXPIRED' USING ERRCODE='40001';END IF;
  IF kind='legacy' THEN
   IF item.status<>'pending' THEN RAISE EXCEPTION 'RESERVATION_NOT_PENDING' USING ERRCODE='40001';END IF;
   IF operation='approve' THEN
    IF NOT cbgpu_accounts.can_user_use_resource(item.owner_id,'gpu','cbsh') OR NOT cbgpu_schools.gpu_notebook_permission(item.owner_id,'cbsh',greatest(item.start_time,clock_timestamp()),item.end_time) THEN RAISE EXCEPTION 'OWNER_GPU_PERMISSION_REQUIRED' USING ERRCODE='42501';END IF;
    IF EXISTS(SELECT FROM public.reservations b WHERE b.gpu_id::text=item.gpu_id AND b.id<>item.id AND b.status='approved' AND b.start_time<=clock_timestamp() AND b.end_time>clock_timestamp()) THEN RAISE EXCEPTION 'GPU_CURRENTLY_IN_USE' USING ERRCODE='40001';END IF;
   END IF;
   -- Reuse legacy approval's atomic status/password pattern, without giving the
   -- school actor platform admin status or bypassing existing overlap triggers.
   claims:=current_setting('request.jwt.claims',true);
   PERFORM set_config('request.jwt.claims',(coalesce(claims,'{}')::jsonb||jsonb_build_object('role','service_role'))::text,true);
   UPDATE public.reservations SET status=CASE operation WHEN 'approve' THEN 'approved' ELSE 'rejected' END WHERE id=item.id;
   PERFORM set_config('request.jwt.claims',claims,true);
   IF operation='approve' THEN
    INSERT INTO public.gpus(id,password) VALUES(item.gpu_id::int,encode(extensions.gen_random_bytes(16),'hex')) ON CONFLICT(id) DO UPDATE SET password=excluded.password;
   END IF;
  ELSE
   IF item.status<>'RESERVED' THEN RAISE EXCEPTION 'RESERVATION_NOT_RESERVED' USING ERRCODE='40001';END IF;
   UPDATE cbgpu_schools.gpu_resource_reservations SET status=CASE operation WHEN 'cancel' THEN 'CANCELLED' ELSE 'REVOKED' END WHERE id=item.id;
   SELECT id INTO job_id FROM cbgpu_schools.gpu_notebook_jobs WHERE reservation_id=item.id FOR UPDATE;
   IF job_id IS NOT NULL THEN PERFORM cbgpu_schools.gpu_notebook_stop(job_id,'SCHOOL_ADMIN_'||upper(operation));END IF;
  END IF;
  INSERT INTO cbgpu_schools.audit(actor_id,school_id,action,target_id,before_data,after_data) VALUES(uid,p_school_id,'GPU_RESERVATION_'||upper(operation),item.id,jsonb_build_object('source',kind,'status',item.status,'end_time',item.end_time),jsonb_build_object('status',CASE operation WHEN 'approve' THEN 'approved' WHEN 'reject' THEN 'rejected' WHEN 'cancel' THEN 'CANCELLED' ELSE 'REVOKED' END));
  RETURN jsonb_build_object('school_id',p_school_id,'reservation',(SELECT cbgpu_schools.gpu_admin_booking_item(p_school_id,to_jsonb(b)) FROM cbgpu_schools.gpu_admin_booking_rows(p_school_id) b WHERE b.id=item.id AND b.source=kind),'server_now',clock_timestamp());
 END IF;
 RAISE EXCEPTION 'INVALID_ACTION' USING ERRCODE='22023';
END$$;
REVOKE ALL ON FUNCTION public.school_gpu_reservation_admin(text,text,jsonb) FROM PUBLIC,anon,authenticated,service_role;
GRANT EXECUTE ON FUNCTION public.school_gpu_reservation_admin(text,text,jsonb) TO authenticated;
COMMIT;
