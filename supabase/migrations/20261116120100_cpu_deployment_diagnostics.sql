-- Candidate additive schema only. Apply through the backend migration workflow after review.
-- Existing grants may include table-wide UPDATE: this trigger independently protects diagnostics.
BEGIN;
ALTER TABLE public.cpu_projects
  ADD COLUMN IF NOT EXISTS deployment_logs text,
  ADD COLUMN IF NOT EXISTS deployment_error text,
  ADD COLUMN IF NOT EXISTS deployment_attempted_at timestamptz,
  ADD COLUMN IF NOT EXISTS deployment_diagnostics jsonb;
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid='public.cpu_projects'::regclass AND conname='cpu_deployment_logs_bounded') THEN
    ALTER TABLE public.cpu_projects ADD CONSTRAINT cpu_deployment_logs_bounded CHECK (char_length(deployment_logs)<=200000) NOT VALID;
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid='public.cpu_projects'::regclass AND conname='cpu_deployment_error_bounded') THEN
    ALTER TABLE public.cpu_projects ADD CONSTRAINT cpu_deployment_error_bounded CHECK (char_length(deployment_error)<=2000) NOT VALID;
  END IF;
END $$;
ALTER TABLE public.cpu_projects VALIDATE CONSTRAINT cpu_deployment_logs_bounded;
ALTER TABLE public.cpu_projects VALIDATE CONSTRAINT cpu_deployment_error_bounded;
CREATE OR REPLACE FUNCTION public.cbgpu_guard_cpu_diagnostics() RETURNS trigger
LANGUAGE plpgsql SECURITY INVOKER SET search_path=pg_catalog AS $$
DECLARE owner_role name;
BEGIN
  SELECT pg_get_userbyid(relowner) INTO owner_role FROM pg_class WHERE oid=TG_RELID;
  IF current_user::text NOT IN ('service_role',owner_role::text) THEN
    IF TG_OP='INSERT' THEN
      IF NEW.deployment_logs IS NOT NULL OR NEW.deployment_error IS NOT NULL OR NEW.deployment_attempted_at IS NOT NULL OR NEW.deployment_diagnostics IS NOT NULL THEN
        RAISE EXCEPTION USING ERRCODE='42501',MESSAGE='Deployment diagnostics are service managed';
      END IF;
    ELSIF NEW.deployment_logs IS DISTINCT FROM OLD.deployment_logs OR NEW.deployment_error IS DISTINCT FROM OLD.deployment_error OR NEW.deployment_attempted_at IS DISTINCT FROM OLD.deployment_attempted_at OR NEW.deployment_diagnostics IS DISTINCT FROM OLD.deployment_diagnostics THEN
      RAISE EXCEPTION USING ERRCODE='42501',MESSAGE='Deployment diagnostics are service managed';
    END IF;
  END IF;
  RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION public.cbgpu_guard_cpu_diagnostics() FROM PUBLIC;
DROP TRIGGER IF EXISTS cbgpu_guard_cpu_diagnostics ON public.cpu_projects;
CREATE TRIGGER cbgpu_guard_cpu_diagnostics BEFORE INSERT OR UPDATE ON public.cpu_projects FOR EACH ROW EXECUTE FUNCTION public.cbgpu_guard_cpu_diagnostics();
COMMIT;
