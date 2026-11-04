DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='cbgpu_developer') THEN CREATE ROLE cbgpu_developer LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION BYPASSRLS CONNECTION LIMIT 3; END IF; END $$;
ALTER ROLE cbgpu_developer SET statement_timeout='8s';
ALTER ROLE cbgpu_developer SET lock_timeout='2s';
GRANT USAGE ON SCHEMA public TO cbgpu_developer;
GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA public TO cbgpu_developer;
GRANT USAGE,SELECT,UPDATE ON ALL SEQUENCES IN SCHEMA public TO cbgpu_developer;
CREATE SCHEMA IF NOT EXISTS developer_workspace AUTHORIZATION cbgpu_developer;
CREATE SCHEMA IF NOT EXISTS developer_console;
REVOKE ALL ON SCHEMA developer_console FROM PUBLIC;
GRANT USAGE ON SCHEMA developer_console TO cbgpu_developer;
CREATE OR REPLACE FUNCTION developer_console.run_query(query_text text) RETURNS jsonb
LANGUAGE plpgsql SECURITY INVOKER SET search_path=public,pg_catalog AS $$
DECLARE row_data record; output jsonb='[]'::jsonb; changed bigint=0; columns_json jsonb='[]'::jsonb;
BEGIN
 IF octet_length(query_text)>20000 THEN RAISE EXCEPTION 'Query exceeds 20 KB'; END IF;
 IF query_text ~* '^\s*(select|with|explain|show|values)\M' OR query_text ~* '\mreturning\M' THEN
  FOR row_data IN EXECUTE query_text LOOP
   IF changed=0 THEN SELECT coalesce(jsonb_agg(key),'[]'::jsonb) INTO columns_json FROM jsonb_each(to_jsonb(row_data)); END IF;
   output=output||jsonb_build_array(to_jsonb(row_data)); changed=changed+1;
   EXIT WHEN changed>=500;
  END LOOP;
 ELSE
  EXECUTE query_text; GET DIAGNOSTICS changed=ROW_COUNT;
 END IF;
 RETURN jsonb_build_object('columns',columns_json,'rows',output,'row_count',changed);
END $$;
REVOKE ALL ON FUNCTION developer_console.run_query(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION developer_console.run_query(text) TO cbgpu_developer;
