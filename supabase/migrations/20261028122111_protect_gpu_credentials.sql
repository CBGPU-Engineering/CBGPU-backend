BEGIN;
DROP POLICY IF EXISTS "Allow public read" ON public.gpus;
CREATE OR REPLACE FUNCTION public.get_active_gpu_password(p_gpu_id integer)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path = '' AS $$
BEGIN
 IF auth.uid() IS NULL OR NOT EXISTS (
  SELECT 1 FROM public.reservations r WHERE r.gpu_id=p_gpu_id AND r.user_id=auth.uid()
  AND r.status='approved' AND r.start_time<=now() AND r.end_time>now()
 ) THEN RETURN NULL; END IF;
 RETURN (SELECT g.password FROM public.gpus g WHERE g.id=p_gpu_id);
END;
$$;
REVOKE ALL ON FUNCTION public.get_active_gpu_password(integer) FROM PUBLIC,anon;
GRANT EXECUTE ON FUNCTION public.get_active_gpu_password(integer) TO authenticated,service_role;
DROP POLICY IF EXISTS "Allow anon to read all reservations" ON public.reservations;
COMMIT;
