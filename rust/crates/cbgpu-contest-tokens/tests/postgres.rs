use axum::{body::Body, http::Request};
use cbgpu_contest_tokens::{
    MintRequest, Persistence, RegistrationInput, Verified, generate, store::Store,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tower::ServiceExt;
const OWNER: &str = "00000000-0000-4000-8000-000000000002";
const OTHER: &str = "00000000-0000-4000-8000-000000000003";
fn id(n: u8) -> uuid::Uuid {
    format!("60000000-0000-4000-8000-{n:012}").parse().unwrap()
}
fn registration(kind: &str) -> RegistrationInput {
    RegistrationInput {
        attendee_name: "Fixture participant".into(),
        answers: json!({"21000000-0000-4000-8000-000000000001":"study"}),
        kind: kind.into(),
        team_name: None,
        join_code: None,
    }
}
#[tokio::test]
#[ignore = "Requires disposable CBGPU_CONTEST_TOKEN_TEST_DATABASE_URL ending /cbgpu_contest_tokens_test and CBGPU_CONTEST_REPO"]
async fn scoped_grants_preserve_existing_rpc_rules_and_reject_expiry_revocation_cross_owner_and_raw_privilege()
 {
    let url = std::env::var("CBGPU_CONTEST_TOKEN_TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/cbgpu_contest_tokens_test"));
    let repo = std::path::PathBuf::from(std::env::var("CBGPU_CONTEST_REPO").unwrap());
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let mut sources = vec![];
    for file in [
        "schema/tests/contests/bootstrap.sql",
        "schema/legacy/web-supabase/migrations/20261018051657_contest_submissions.sql",
        "schema/legacy/web-supabase/migrations/20261018052904_contest_logos.sql",
        "schema/legacy/web-supabase/migrations/20261018114551_event_management.sql",
        "schema/legacy/web-supabase/migrations/20261018120743_event_operations.sql",
        "supabase/migrations/20261116080117_contest_presentation_assets.sql",
        "supabase/migrations/20261116083445_contest_team_registration.sql",
        "supabase/migrations/20261116103228_contest_start_windows.sql",
    ] {
        let bytes = std::fs::read(repo.join(file)).unwrap();
        sources.push((file, Sha256::digest(&bytes)));
        sqlx::raw_sql(sqlx::AssertSqlSafe(std::str::from_utf8(&bytes).unwrap()))
            .execute(&admin)
            .await
            .unwrap();
    }
    sqlx::raw_sql("ALTER TABLE auth.users ADD COLUMN deleted_at timestamptz,ADD COLUMN banned_until timestamptz,ADD COLUMN encrypted_password text;ALTER TABLE auth.users ENABLE ROW LEVEL SECURITY;CREATE TABLE auth.sessions(id uuid PRIMARY KEY,user_id uuid,not_after timestamptz,secret text);ALTER TABLE auth.sessions ENABLE ROW LEVEL SECURITY;CREATE TABLE public.profiles(id uuid PRIMARY KEY,is_approved boolean,account_status text);ALTER TABLE public.profiles ENABLE ROW LEVEL SECURITY;CREATE FUNCTION auth.jwt()RETURNS jsonb LANGUAGE sql STABLE AS $$SELECT coalesce(nullif(current_setting('request.jwt.claims',true),'')::jsonb,'{}'::jsonb)$$;").execute(&admin).await.unwrap();
    for file in [
        "supabase/migrations/20261116130154_contest_external_api_session_guard.sql",
        "supabase/migrations/20261116131013_contest_scoped_automation_tokens.sql",
    ] {
        let bytes = std::fs::read(repo.join(file)).unwrap();
        sources.push((file, Sha256::digest(&bytes)));
        for _ in 0..2 {
            sqlx::raw_sql(sqlx::AssertSqlSafe(std::str::from_utf8(&bytes).unwrap()))
                .execute(&admin)
                .await
                .unwrap_or_else(|e| panic!("{file}: {e}"));
        }
    }
    for role in ["anon", "authenticated"] {
        let mut tx = admin.begin().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("SET LOCAL ROLE {role}")))
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(
            sqlx::query("SELECT contest_api.list_owned()")
                .fetch_all(&mut *tx)
                .await
                .is_err()
        );
        tx.rollback().await.unwrap();
    }
    let third: uuid::Uuid = "00000000-0000-4000-8000-000000000004".parse().unwrap();
    let third_session: uuid::Uuid = "10000000-0000-4000-8000-000000000004".parse().unwrap();
    let owner: uuid::Uuid = OWNER.parse().unwrap();
    let other: uuid::Uuid = OTHER.parse().unwrap();
    let host: uuid::Uuid = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let owner_session: uuid::Uuid = "10000000-0000-4000-8000-000000000002".parse().unwrap();
    let other_session: uuid::Uuid = "10000000-0000-4000-8000-000000000003".parse().unwrap();
    for (user, session) in [
        (
            host,
            "10000000-0000-4000-8000-000000000001"
                .parse::<uuid::Uuid>()
                .unwrap(),
        ),
        (owner, owner_session),
        (other, other_session),
        (third, third_session),
    ] {
        sqlx::query(
            "INSERT INTO auth.users(id,encrypted_password)VALUES($1,'unchanged-password-hash')",
        )
        .bind(user)
        .execute(&admin)
        .await
        .unwrap();
        sqlx::query("INSERT INTO public.profiles VALUES($1,true,NULL)")
            .bind(user)
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("INSERT INTO auth.sessions VALUES($1,$2,NULL,'private-refresh-state')")
            .bind(session)
            .bind(user)
            .execute(&admin)
            .await
            .unwrap();
    }
    let unchanged: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(u)FROM auth.users u ORDER BY id")
            .fetch_all(&admin)
            .await
            .unwrap();
    for n in 1..=5 {
        sqlx::query("INSERT INTO public.contests(id,host_id,title,fields,registration_questions,capacity,waitlist_enabled,event_start_at,registration_closes_at,registration_opens_at,participation_mode,team_min_size,team_max_size,allow_individual_entries)VALUES($1,$2,$3,'[{\"id\":\"20000000-0000-4000-8000-000000000001\",\"type\":\"text\",\"label\":\"Report\",\"description\":\"\",\"required\":false}]','[{\"id\":\"21000000-0000-4000-8000-000000000001\",\"type\":\"text\",\"label\":\"Reason\",\"required\":true}]',$4,$5,clock_timestamp()+interval'3 hours',clock_timestamp()+CASE WHEN $6 THEN interval'-1 hour'ELSE interval'2 hours'END,CASE WHEN $7 THEN clock_timestamp()+interval'1 hour'ELSE NULL END,$8,2,2,true)").bind(id(n)).bind(host).bind(format!("Contest{n}")).bind(if n==1||n==5{Some(1i32)}else{None}).bind(n!=5).bind(n==4).bind(n==3).bind(if n==2{"team"}else{"individual"}).execute(&admin).await.unwrap();
    }
    // These future private columns must never appear in token read projections.
    sqlx::raw_sql("ALTER TABLE public.contests ADD COLUMN internal_secret text DEFAULT'not-for-token';ALTER TABLE public.event_registrations ADD COLUMN internal_secret text DEFAULT'not-for-token';").execute(&admin).await.unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE cbgpu_contest_tokens_backend")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    let store = Store { pool };
    cbgpu_contest_tokens::store::verify_executor(&store.pool)
        .await
        .unwrap();
    assert!(
        cbgpu_contest_tokens::store::verify_executor(&admin)
            .await
            .is_err(),
        "broad bootstrap credentials cannot start the executor"
    );
    // Restore/ACL drift must fail startup before any delegated request.
    sqlx::raw_sql("ALTER FUNCTION contest_api._policy_contest(uuid)OWNER TO postgres")
        .execute(&admin)
        .await
        .unwrap();
    assert!(
        cbgpu_contest_tokens::store::verify_executor(&store.pool)
            .await
            .is_err()
    );
    sqlx::raw_sql("GRANT CREATE ON SCHEMA contest_api TO cbgpu_contest_policy_reader;ALTER FUNCTION contest_api._policy_contest(uuid)OWNER TO cbgpu_contest_policy_reader;REVOKE CREATE ON SCHEMA contest_api FROM cbgpu_contest_policy_reader").execute(&admin).await.unwrap();
    cbgpu_contest_tokens::store::verify_executor(&store.pool)
        .await
        .unwrap();
    sqlx::raw_sql("GRANT UPDATE ON public.event_registrations TO cbgpu_contest_tokens_backend")
        .execute(&admin)
        .await
        .unwrap();
    assert!(
        cbgpu_contest_tokens::store::verify_executor(&store.pool)
            .await
            .is_err()
    );
    sqlx::raw_sql("REVOKE UPDATE ON public.event_registrations FROM cbgpu_contest_tokens_backend")
        .execute(&admin)
        .await
        .unwrap();
    cbgpu_contest_tokens::store::verify_executor(&store.pool)
        .await
        .unwrap();
    sqlx::raw_sql("REVOKE EXECUTE ON FUNCTION contest_api.cancel_allowed(bytea,uuid)FROM cbgpu_contest_tokens_backend").execute(&admin).await.unwrap();
    assert!(
        cbgpu_contest_tokens::store::verify_executor(&store.pool)
            .await
            .is_err()
    );
    sqlx::raw_sql("GRANT EXECUTE ON FUNCTION contest_api.cancel_allowed(bytea,uuid)TO cbgpu_contest_tokens_backend").execute(&admin).await.unwrap();
    cbgpu_contest_tokens::store::verify_executor(&store.pool)
        .await
        .unwrap();
    let forbidden = [
        "SELECT * FROM contest_api.tokens",
        "SELECT id FROM auth.users",
        "SELECT id FROM auth.sessions",
        "SELECT id FROM public.profiles",
        "INSERT INTO public.event_registrations(contest_id,user_id,attendee_name)VALUES(NULL,NULL,'forbidden')",
        "SELECT public.register_contest_entry(NULL,'forbidden')",
        "SELECT contest_api._authorize(NULL,'contests:read',NULL)",
        "SELECT contest_api._policy_visible(NULL)",
        "SELECT contest_api._policy_contest(NULL)",
    ];
    for sql in forbidden {
        assert!(
            sqlx::query(sql).execute(&store.pool).await.is_err(),
            "narrow role must reject {sql}"
        );
    }
    let elevated:bool=sqlx::query_scalar("SELECT rolsuper OR rolbypassrls OR rolcanlogin OR rolcreaterole OR rolcreatedb OR pg_has_role(rolname,'authenticated','MEMBER')FROM pg_roles WHERE rolname='cbgpu_contest_tokens_backend'").fetch_one(&admin).await.unwrap();
    assert!(!elevated);
    let actor = Verified {
        subject: owner,
        session: owner_session,
    };
    let other_actor = Verified {
        subject: other,
        session: other_session,
    };
    let input = MintRequest {
        name: "Scoped fixture".into(),
        scopes: vec![
            "contests:read".into(),
            "contests:register".into(),
            "contests:cancel".into(),
        ],
        allowed_contest_ids: (1..=5).map(id).collect(),
        expires_in_days: 30,
    };
    let (secret, hash) = generate();
    let record = store.mint(&actor, &hash, &input).await.unwrap();
    let token_id: uuid::Uuid = record["id"].as_str().unwrap().parse().unwrap();
    assert!(record.get("token_sha256").is_none());
    assert!(record.get("parent_session").is_none());
    let raw: String =
        sqlx::query_scalar("SELECT to_jsonb(t)::text FROM contest_api.tokens t WHERE id=$1")
            .bind(token_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(!raw.contains(&secret));
    let saved_hash: Vec<u8> =
        sqlx::query_scalar("SELECT token_sha256 FROM contest_api.tokens WHERE id=$1")
            .bind(token_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(saved_hash, hash.bytes());
    assert_eq!(store.list(&other_actor).await.unwrap()["tokens"], json!([]));
    assert!(!store.revoke(&other_actor, token_id).await.unwrap());
    let claims: Option<String> =
        sqlx::query_scalar("SELECT current_setting('request.jwt.claims',true)")
            .fetch_one(&store.pool)
            .await
            .unwrap();
    assert!(claims.unwrap_or_default().is_empty());
    let public = store.read(&hash, None, false).await.unwrap();
    assert_eq!(public["contests"].as_array().unwrap().len(), 5);
    assert!(!public.to_string().contains("not-for-token"));
    let mut missing = registration("individual");
    missing.answers = json!({});
    assert_eq!(
        store
            .register(&hash, id(1), &missing)
            .await
            .unwrap_err()
            .status,
        400
    );
    let first = store
        .register(&hash, id(1), &registration("individual"))
        .await
        .unwrap();
    assert_eq!(first["registration"]["status"], "confirmed");
    assert_eq!(first["registration"]["user_id"], OWNER);
    assert!(!first.to_string().contains("not-for-token"));
    assert_eq!(
        store
            .register(&hash, id(1), &registration("individual"))
            .await
            .unwrap_err()
            .status,
        409
    );
    // The policy-reader must follow both current unlisted access and a stricter future RLS policy.
    for (n, resource_owner, visibility) in [
        (6, host, "unlisted"),
        (7, host, "public"),
        (8, owner, "public"),
    ] {
        sqlx::query("INSERT INTO public.contests(id,host_id,title,fields,registration_questions,event_start_at,registration_closes_at,visibility)SELECT $1,$2,$3,fields,registration_questions,event_start_at,registration_closes_at,$4 FROM public.contests WHERE id=$5").bind(id(n)).bind(resource_owner).bind(format!("Policy fixture{n}")).bind(visibility).bind(id(1)).execute(&admin).await.unwrap();
    }
    let (_, unlisted_hash) = generate();
    let unlisted_input = MintRequest {
        scopes: vec!["contests:read".into()],
        allowed_contest_ids: vec![id(6)],
        ..input.clone()
    };
    store
        .mint(&actor, &unlisted_hash, &unlisted_input)
        .await
        .unwrap();
    assert_eq!(
        store
            .read(&unlisted_hash, Some(id(6)), false)
            .await
            .unwrap()["contest"]["visibility"],
        "unlisted"
    );
    let (hidden_secret, hidden_hash) = generate();
    let hidden_input = MintRequest {
        allowed_contest_ids: vec![id(7)],
        ..input.clone()
    };
    store
        .mint(&actor, &hidden_hash, &hidden_input)
        .await
        .unwrap();
    store
        .register(&hidden_hash, id(7), &registration("individual"))
        .await
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("ALTER POLICY contests_read ON public.contests USING(host_id=auth.uid()OR id NOT IN('{}'::uuid,'{}'::uuid))",id(7),id(8)))).execute(&admin).await.unwrap();
    let (_, blocked_hash) = generate();
    assert_eq!(
        store
            .mint(&actor, &blocked_hash, &hidden_input)
            .await
            .unwrap_err()
            .status,
        404,
        "known but policy-hidden ID must not become a grant"
    );
    assert_eq!(
        store
            .read(&hidden_hash, Some(id(7)), false)
            .await
            .unwrap_err()
            .status,
        404,
        "existing grants must recheck changed policy"
    );
    assert_eq!(
        store.read(&hidden_hash, None, false).await.unwrap()["contests"],
        json!([])
    );
    assert_eq!(
        store
            .register(&hidden_hash, id(7), &registration("individual"))
            .await
            .unwrap_err()
            .status,
        404
    );
    struct NoJwt;
    #[async_trait::async_trait]
    impl cbgpu_contest_tokens::Identity for NoJwt {
        async fn authenticate(
            &self,
            _: &str,
        ) -> cbgpu_contest_tokens::Result<cbgpu_contests_api::Principal> {
            panic!("PAT does not acquire JWT privileges")
        }
    }
    let candidate = cbgpu_contest_tokens::api::Api::new(
        Arc::new(NoJwt),
        Arc::new(Store {
            pool: store.pool.clone(),
        }),
        b"private-policy-fixture-key-at-least32bytes".to_vec(),
    )
    .unwrap();
    let form = cbgpu_contest_tokens::api::router(candidate)
        .oneshot(
            Request::builder()
                .uri(format!("/v1/contests/{}/form", id(7)))
                .header("authorization", format!("Bearer {hidden_secret}"))
                .header(
                    "x-cbgpu-contest-key",
                    "private-policy-fixture-key-at-least32bytes",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        form.status(),
        404,
        "real form route must not leak changed-policy metadata"
    );
    assert_eq!(
        store.read(&hidden_hash, Some(id(7)), true).await.unwrap()["registration"]["user_id"],
        OWNER,
        "own entry remains available after metadata hiding"
    );
    assert_eq!(
        store.cancel(&hidden_hash, id(7)).await.unwrap()["registration"]["status"],
        "cancelled"
    );
    let (_, own_hidden_hash) = generate();
    let own_hidden = MintRequest {
        allowed_contest_ids: vec![id(8)],
        ..unlisted_input.clone()
    };
    store
        .mint(&actor, &own_hidden_hash, &own_hidden)
        .await
        .unwrap();
    assert_eq!(
        store
            .read(&own_hidden_hash, Some(id(8)), false)
            .await
            .unwrap()["contest"]["id"],
        id(8).to_string(),
        "host follows its actual RLS visibility"
    );
    assert_eq!(
        store
            .mint(&other_actor, &blocked_hash, &own_hidden)
            .await
            .unwrap_err()
            .status,
        404
    );
    let helper_isolated:bool=sqlx::query_scalar("SELECT NOT pg_has_role('cbgpu_contest_tokens_backend','cbgpu_contest_policy_reader','MEMBER')AND NOT r.rolsuper AND NOT r.rolbypassrls AND NOT r.rolcanlogin FROM pg_roles r WHERE rolname='cbgpu_contest_policy_reader'").fetch_one(&admin).await.unwrap();
    assert!(helper_isolated);
    sqlx::raw_sql("ALTER POLICY contests_read ON public.contests USING(true)")
        .execute(&admin)
        .await
        .unwrap();
    let (_, other_hash) = generate();
    store.mint(&other_actor, &other_hash, &input).await.unwrap();
    let waiting = store
        .register(&other_hash, id(1), &registration("individual"))
        .await
        .unwrap();
    assert_eq!(waiting["registration"]["status"], "waitlisted");
    assert_eq!(waiting["registration"]["user_id"], OTHER);
    assert_eq!(
        store
            .register(&hash, id(3), &registration("individual"))
            .await
            .unwrap_err()
            .status,
        409
    );
    assert_eq!(
        store
            .register(&hash, id(4), &registration("individual"))
            .await
            .unwrap_err()
            .status,
        409
    );
    store
        .register(&hash, id(5), &registration("individual"))
        .await
        .unwrap();
    assert_eq!(
        store
            .register(&other_hash, id(5), &registration("individual"))
            .await
            .unwrap_err()
            .status,
        409
    );
    let mut team = registration("team_create");
    team.team_name = Some("First team".into());
    store.register(&hash, id(2), &team).await.unwrap();
    let code: uuid::Uuid =
        sqlx::query_scalar("SELECT join_code FROM public.contest_teams WHERE contest_id=$1")
            .bind(id(2))
            .fetch_one(&admin)
            .await
            .unwrap();
    let mut join = registration("team_join");
    join.join_code = Some(code);
    store.register(&other_hash, id(2), &join).await.unwrap();
    let (_, third_hash) = generate();
    store
        .mint(
            &Verified {
                subject: third,
                session: third_session,
            },
            &third_hash,
            &input,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .register(&third_hash, id(2), &join)
            .await
            .unwrap_err()
            .status,
        400,
        "team maximum enforced by unchanged RPC"
    );
    let members: i64 =
        sqlx::query_scalar("SELECT count(*)FROM public.contest_team_members WHERE contest_id=$1")
            .bind(id(2))
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(members, 2);
    let own = store.read(&hash, Some(id(1)), true).await.unwrap();
    assert_eq!(own["registration"]["user_id"], OWNER);
    let cancel = store.cancel(&hash, id(1)).await.unwrap();
    assert_eq!(cancel["registration"]["status"], "cancelled");
    assert_eq!(
        store.read(&other_hash, Some(id(1)), true).await.unwrap()["registration"]["status"],
        "confirmed" // Existing cancellation trigger promotes the waitlisted participant.
    );
    assert!(cancel["registration"]["answers"].is_object());
    assert!(
        cancel["registration"]["ticket_code"].is_string(),
        "read+cancel retains full own response"
    );
    sqlx::query("INSERT INTO public.contests(id,host_id,title,fields,registration_questions,event_start_at,registration_closes_at,capacity,waitlist_enabled)SELECT $1,host_id,'Write-only receipt fixture',fields,registration_questions,event_start_at,registration_closes_at,1,true FROM public.contests WHERE id=$2").bind(id(9)).bind(id(1)).execute(&admin).await.unwrap();
    let (_, write_hash) = generate();
    let write_input = MintRequest {
        scopes: vec!["contests:register".into()],
        allowed_contest_ids: vec![id(9)],
        ..input.clone()
    };
    store.mint(&actor, &write_hash, &write_input).await.unwrap();
    let receipt = store
        .register(&write_hash, id(9), &registration("individual"))
        .await
        .unwrap();
    assert_eq!(receipt["registration"]["status"], "confirmed");
    assert_eq!(receipt["registration"].as_object().unwrap().len(), 4);
    for key in ["answers", "ticket_code", "user_id", "attendee_name"] {
        assert!(
            receipt["registration"].get(key).is_none(),
            "register-only must not disclose {key}"
        );
    }
    let (_, waiting_hash) = generate();
    store
        .mint(&other_actor, &waiting_hash, &write_input)
        .await
        .unwrap();
    assert_eq!(
        store
            .register(&waiting_hash, id(9), &registration("individual"))
            .await
            .unwrap()["registration"]["status"],
        "waitlisted"
    );
    let (_, cancel_hash) = generate();
    let cancel_input = MintRequest {
        scopes: vec!["contests:cancel".into()],
        ..write_input.clone()
    };
    store
        .mint(&actor, &cancel_hash, &cancel_input)
        .await
        .unwrap();
    let cancelled = store.cancel(&cancel_hash, id(9)).await.unwrap();
    assert_eq!(cancelled["registration"]["status"], "cancelled");
    assert_eq!(cancelled["registration"].as_object().unwrap().len(), 4);
    for key in ["answers", "ticket_code", "user_id", "attendee_name"] {
        assert!(
            cancelled["registration"].get(key).is_none(),
            "cancel-only must not disclose {key}"
        );
    }
    assert_eq!(
        store
            .read(&cancel_hash, Some(id(9)), true)
            .await
            .unwrap_err()
            .status,
        403
    );
    let (_, other_read_hash) = generate();
    let other_read = MintRequest {
        scopes: vec!["contests:read".into()],
        ..write_input
    };
    store
        .mint(&other_actor, &other_read_hash, &other_read)
        .await
        .unwrap();
    assert_eq!(
        store
            .read(&other_read_hash, Some(id(9)), true)
            .await
            .unwrap()["registration"]["status"],
        "confirmed",
        "cancel-only still triggers existing waitlist promotion"
    );
    let (_, read_hash) = generate();
    let read_input = MintRequest {
        scopes: vec!["contests:read".into()],
        allowed_contest_ids: vec![id(1)],
        ..input.clone()
    };
    store.mint(&actor, &read_hash, &read_input).await.unwrap();
    assert_eq!(
        store
            .read(&read_hash, Some(id(2)), false)
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        store
            .register(&read_hash, id(1), &registration("individual"))
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        store.cancel(&read_hash, id(1)).await.unwrap_err().status,
        403
    );
    let (_, invalid) = generate();
    let too_long = MintRequest {
        expires_in_days: 91,
        ..input.clone()
    };
    assert_eq!(
        store
            .mint(&actor, &invalid, &too_long)
            .await
            .unwrap_err()
            .status,
        400
    );
    sqlx::query("UPDATE contest_api.tokens SET created_at=clock_timestamp()-interval'2 days',expires_at=clock_timestamp()-interval'1 day'WHERE id=$1").bind(token_id).execute(&admin).await.unwrap();
    assert_eq!(
        store.read(&hash, None, false).await.unwrap_err().status,
        401
    );
    sqlx::query(
        "UPDATE contest_api.tokens SET expires_at=clock_timestamp()+interval'30 days'WHERE id=$1",
    )
    .bind(token_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("UPDATE public.profiles SET is_approved=false WHERE id=$1")
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    assert!(store.read(&hash, None, false).await.is_ok());
    assert_eq!(
        store
            .register(&hash, id(1), &registration("individual"))
            .await
            .unwrap_err()
            .status,
        403
    );
    sqlx::query("UPDATE public.profiles SET is_approved=true WHERE id=$1")
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE FUNCTION public.cbgpu_can_use_resource(resource text,target_school text DEFAULT'cbsh')RETURNS boolean LANGUAGE sql AS $$SELECT false$$").execute(&admin).await.unwrap();
    assert_eq!(
        store
            .register(&hash, id(1), &registration("individual"))
            .await
            .unwrap_err()
            .status,
        403,
        "installed resource policy overrides legacy approval freshly"
    );
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION public.cbgpu_can_use_resource(resource text,target_school text DEFAULT'cbsh')RETURNS boolean LANGUAGE sql AS $$SELECT resource='contests'AND target_school='cbsh'AND auth.uid()='00000000-0000-4000-8000-000000000002'::uuid$$").execute(&admin).await.unwrap();
    sqlx::query("UPDATE public.profiles SET is_approved=false WHERE id=$1")
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        store
            .register(&hash, id(1), &registration("individual"))
            .await
            .unwrap()["registration"]["status"],
        "waitlisted",
        "valid future guest resource grant works without changing account type"
    );
    sqlx::raw_sql("DROP FUNCTION public.cbgpu_can_use_resource(text,text)")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE public.profiles SET is_approved=true,account_status='SUSPENDED'WHERE id=$1",
    )
    .bind(owner)
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        store.read(&hash, None, false).await.unwrap_err().status,
        401
    );
    sqlx::query("UPDATE public.profiles SET account_status=NULL WHERE id=$1")
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE auth.users SET banned_until=clock_timestamp()+interval'1 hour'WHERE id=$1")
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        store.read(&hash, None, false).await.unwrap_err().status,
        401
    );
    sqlx::query("UPDATE auth.users SET banned_until=NULL WHERE id=$1")
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE auth.users SET deleted_at=clock_timestamp()WHERE id=$1")
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        store.read(&hash, None, false).await.unwrap_err().status,
        401
    );
    sqlx::query("UPDATE auth.users SET deleted_at=NULL WHERE id=$1")
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE auth.sessions SET not_after=clock_timestamp()-interval'1 second'WHERE id=$1",
    )
    .bind(owner_session)
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        store.read(&hash, None, false).await.unwrap_err().status,
        401
    );
    sqlx::query("UPDATE auth.sessions SET not_after=NULL WHERE id=$1")
        .bind(owner_session)
        .execute(&admin)
        .await
        .unwrap();
    let active:i64=sqlx::query_scalar("SELECT count(*)FROM contest_api.tokens WHERE owner=$1 AND revoked_at IS NULL AND expires_at>clock_timestamp()").bind(owner).fetch_one(&admin).await.unwrap();
    for index in 0..(20 - active) {
        let (_, new_hash) = generate();
        let quota_input = MintRequest {
            expires_in_days: if index == 0 { 90 } else { 30 },
            ..read_input.clone()
        };
        store.mint(&actor, &new_hash, &quota_input).await.unwrap();
    }
    let (_, over_quota) = generate();
    assert_eq!(
        store
            .mint(&actor, &over_quota, &read_input)
            .await
            .unwrap_err()
            .status,
        429
    );
    assert!(store.revoke(&actor, token_id).await.unwrap());
    assert_eq!(
        store.read(&hash, None, false).await.unwrap_err().status,
        401
    );
    sqlx::query("DELETE FROM auth.sessions WHERE id=$1")
        .bind(owner_session)
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        store
            .read(&read_hash, None, false)
            .await
            .unwrap_err()
            .status,
        401
    );
    assert!(store.mint(&actor, &invalid, &input).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, Value>("SELECT to_jsonb(u)FROM auth.users u ORDER BY id")
            .fetch_all(&admin)
            .await
            .unwrap(),
        unchanged,
        "password hashes/IDs unchanged after fixture ban reversal and token operations"
    );
    for (file, digest) in sources {
        assert_eq!(
            Sha256::digest(std::fs::read(repo.join(file)).unwrap()),
            digest,
            "reviewed SQL source bytes unchanged"
        );
    }
    store.pool.close().await;
    admin.close().await;
}
