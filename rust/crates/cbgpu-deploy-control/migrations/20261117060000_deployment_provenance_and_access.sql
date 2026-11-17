-- Candidate-only additive parity with the reviewed operating controller.
ALTER TABLE deploy_store.deployments ADD COLUMN requested_by text;
ALTER TABLE deploy_store.compose_states ADD COLUMN access_state text CHECK(access_state IS NULL OR access_state IN('ALLOW','DENY','UNAVAILABLE'));
