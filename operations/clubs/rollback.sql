-- Disable entry points/capability without deleting campaigns, clubs, applicants or audit.
BEGIN;
SET LOCAL lock_timeout='3s';
REVOKE EXECUTE ON FUNCTION public.cbgpu_school_clubs_api(text,jsonb) FROM authenticated;
UPDATE cbgpu_schools.feature_registry SET enabled=false WHERE key='clubs';
-- A later representative change (including explicit re-selection) is never undone.
UPDATE cbgpu_schools.schools s SET features=b.before_features,revision=s.revision+1 FROM cbgpu_schools.club_feature_backfill b WHERE s.id=b.school_id AND s.features=b.applied_features AND s.revision=b.applied_revision;
COMMIT;
