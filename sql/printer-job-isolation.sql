BEGIN;
-- A manager can see and update jobs only for their assigned printers.
-- Owners retain their own requests; the central admin panel retains oversight.
ALTER POLICY print_jobs_select ON public.print_jobs USING (
 user_id=(SELECT auth.uid()) OR public.is_admin((SELECT auth.uid())) OR EXISTS (
  SELECT 1 FROM public.printer_managers pm
  WHERE pm.user_id=(SELECT auth.uid()) AND pm.printer_id=coalesce(print_jobs.printer_id,print_jobs.preferred_printer_id)
 )
);
ALTER POLICY print_jobs_update ON public.print_jobs USING (
 user_id=(SELECT auth.uid()) OR public.is_admin((SELECT auth.uid())) OR EXISTS (
  SELECT 1 FROM public.printer_managers pm
  WHERE pm.user_id=(SELECT auth.uid()) AND pm.printer_id=coalesce(print_jobs.printer_id,print_jobs.preferred_printer_id)
 )
) WITH CHECK (
 user_id=(SELECT auth.uid()) OR public.is_admin((SELECT auth.uid())) OR EXISTS (
  SELECT 1 FROM public.printer_managers pm
  WHERE pm.user_id=(SELECT auth.uid()) AND pm.printer_id=coalesce(print_jobs.printer_id,print_jobs.preferred_printer_id)
 )
);
CREATE OR REPLACE FUNCTION public.enforce_print_job_printer_match() RETURNS trigger
LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog,public AS $$
BEGIN
 IF TG_OP='UPDATE' THEN
  IF NEW.printer_id IS NOT DISTINCT FROM OLD.printer_id AND NEW.preferred_printer_id IS NOT DISTINCT FROM OLD.preferred_printer_id THEN RETURN NEW; END IF;
 END IF;
 IF NEW.printer_id IS NOT NULL AND (NEW.preferred_printer_id IS NULL OR NEW.printer_id<>NEW.preferred_printer_id) THEN
  RAISE EXCEPTION 'This print job must use its requested printer.' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS enforce_print_job_printer_match ON public.print_jobs;
CREATE TRIGGER enforce_print_job_printer_match BEFORE INSERT OR UPDATE OF printer_id,preferred_printer_id ON public.print_jobs
 FOR EACH ROW EXECUTE FUNCTION public.enforce_print_job_printer_match();
CREATE OR REPLACE FUNCTION public.start_print_job(p_job_id uuid,p_printer_id integer,p_hours numeric) RETURNS uuid
LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog,public AS $$
DECLARE v_printer public.printers%ROWTYPE; v_job public.print_jobs%ROWTYPE; v_now timestamptz:=now();v_end timestamptz;v_user uuid:=(SELECT auth.uid());
BEGIN
 IF v_user IS NULL OR NOT (public.is_admin(v_user) OR EXISTS(SELECT 1 FROM public.printer_managers WHERE user_id=v_user AND printer_id=p_printer_id)) THEN
  RAISE EXCEPTION 'You are not assigned to this printer.' USING ERRCODE='42501';
 END IF;
 IF p_hours IS NULL OR p_hours<=0 OR p_hours::text IN ('NaN','Infinity','-Infinity') THEN RAISE EXCEPTION 'Enter a positive estimated duration.' USING ERRCODE='22023'; END IF;
 SELECT * INTO v_printer FROM public.printers WHERE id=p_printer_id FOR UPDATE;
 IF NOT FOUND OR v_printer.status<>'idle' OR v_printer.current_job_id IS NOT NULL THEN RAISE EXCEPTION 'This printer is no longer idle. Refresh its queue.' USING ERRCODE='55000'; END IF;
 SELECT * INTO v_job FROM public.print_jobs WHERE id=p_job_id FOR UPDATE;
 IF NOT FOUND OR v_job.status<>'accepted' OR v_job.printer_id IS NOT NULL THEN RAISE EXCEPTION 'This job is no longer waiting. Refresh the queue.' USING ERRCODE='55000'; END IF;
 IF v_job.preferred_printer_id IS DISTINCT FROM p_printer_id THEN RAISE EXCEPTION 'This job was requested for a different printer.' USING ERRCODE='23514'; END IF;
 v_end:=v_now+(p_hours::text||' hours')::interval;
 UPDATE public.print_jobs SET status='printing',printer_id=p_printer_id,started_at=v_now,estimated_end_at=v_end WHERE id=p_job_id;
 UPDATE public.printers SET status='printing',current_job_id=p_job_id,started_at=v_now,estimated_end_at=v_end WHERE id=p_printer_id;
 RETURN p_job_id;
END;
$$;
REVOKE ALL ON FUNCTION public.start_print_job(uuid,integer,numeric) FROM PUBLIC,anon;
GRANT EXECUTE ON FUNCTION public.start_print_job(uuid,integer,numeric) TO authenticated;
-- Older clients must not attach an unrelated job after a zero-row job update.
CREATE OR REPLACE FUNCTION public.enforce_printer_current_job_match() RETURNS trigger
LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog,public AS $$
BEGIN
 IF TG_OP='UPDATE' THEN IF NEW.current_job_id IS NOT DISTINCT FROM OLD.current_job_id THEN RETURN NEW; END IF; END IF;
 IF NEW.current_job_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM public.print_jobs WHERE id=NEW.current_job_id AND printer_id=NEW.id AND preferred_printer_id=NEW.id AND status='printing') THEN
  RAISE EXCEPTION 'This job is not running on this printer.' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END;
$$;
DROP TRIGGER IF EXISTS enforce_printer_current_job_match ON public.printers;
CREATE TRIGGER enforce_printer_current_job_match BEFORE INSERT OR UPDATE OF current_job_id ON public.printers
 FOR EACH ROW EXECUTE FUNCTION public.enforce_printer_current_job_match();
NOTIFY pgrst,'reload schema';
COMMIT;
