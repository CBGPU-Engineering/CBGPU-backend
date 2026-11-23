-- Synthetic catalog scaled to the observed production directory size.
DO $$DECLARE started timestamptz; elapsed numeric; query jsonb; result jsonb; BEGIN
IF (SELECT count(*) FROM cbgpu_schools.schools)<>2479 THEN RAISE EXCEPTION 'FIXTURE_SIZE'; END IF;
FOR query IN SELECT value FROM jsonb_array_elements('[{}, {"q":"설화고등학교"},{"q":"경북드론고등학교"},{"q":"경상북도"},{"q":"충청남도 설화고등학교"},{"q":"설화고등학교","status":"LISTED"}]') LOOP
FOR i IN 1..3 LOOP started:=clock_timestamp(); result:=public.cbgpu_school_api('directory',query);
elapsed:=extract(epoch FROM clock_timestamp()-started)*1000;
RAISE WARNING 'directory fixture query % sample %: % ms',query,i,elapsed;
IF elapsed>200 THEN RAISE EXCEPTION 'DIRECTORY_LATENCY % ms',elapsed; END IF;
IF query->>'q'='설화고등학교' AND query->>'status'='LISTED' AND result->>'total'<>'0' THEN RAISE EXCEPTION 'DUPLICATE_RESURFACED'; END IF;
END LOOP; END LOOP; END$$;
SELECT '2479-row query: six query variants, three samples each under 200ms';
