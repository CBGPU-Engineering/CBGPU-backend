use super::*;
#[tokio::test]
#[ignore = "requires ARCADE_TEST_DATABASE_URL pointing at a fresh disposable database"]
async fn postgres_boundaries() {
    let url = std::env::var("ARCADE_TEST_DATABASE_URL")
        .expect("explicit disposable ARCADE_TEST_DATABASE_URL required");
    let db = PgPool::connect(&url).await.unwrap();
    sqlx::raw_sql("do $$begin create role anon;exception when duplicate_object then null;end$$;do $$begin create role authenticated;exception when duplicate_object then null;end$$;create schema auth;create table auth.users(id uuid primary key,deleted_at timestamptz,banned_until timestamptz,email text);create table public.profiles(id uuid primary key,account_status text);create schema cbgpu_schools;create table cbgpu_schools.schools(id text primary key,display_name text,official_name text);insert into cbgpu_schools.schools values('test-school','Test school','Test school');create table cbgpu_schools.memberships(user_id uuid,school_id text,status text,verified bool,starts_at timestamptz,ends_at timestamptz);").execute(&db).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/20261120090000_arcade.sql"
    ))
    .execute(&db)
    .await
    .unwrap();
    // Reapplying the additive migration must preserve data and avoid duplicate indexes.
    sqlx::raw_sql(include_str!(
        "../migrations/20261120090000_arcade.sql"
    ))
    .execute(&db)
    .await
    .unwrap();
    sqlx::raw_sql("do $$begin create role cbgpu_arcade_service login nosuperuser nocreatedb nocreaterole noreplication nobypassrls;exception when duplicate_object then null;end$$;alter table auth.users enable row level security;alter table public.profiles enable row level security;alter table cbgpu_schools.memberships enable row level security;alter table cbgpu_schools.schools enable row level security;").execute(&db).await.unwrap();
    for _ in 0..2 {
        sqlx::raw_sql(include_str!(
            "../migrations/20261120090001_arcade_service_privileges.sql"
        ))
        .execute(&db)
        .await
        .unwrap();
    }
    let service_db = sqlx::postgres::PgPoolOptions::new()
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("set role cbgpu_arcade_service")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    assert!(
        sqlx::query("select * from auth.users")
            .execute(&service_db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("update public.profiles set account_status='ACTIVE'")
            .execute(&service_db)
            .await
            .is_err()
    );
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    for id in [a, b] {
        sqlx::query("insert into auth.users(id) values($1)")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("insert into profiles values($1,'ACTIVE')")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
    }
    let auth = Router::new().route(
        "/auth/v1/user",
        get(move |h: HeaderMap| async move {
            Json(json!({"id":if h.get("authorization").unwrap()=="Bearer b"{b}else{a}}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, auth).await.unwrap() });
    let s = App {
        db: service_db,
        http: reqwest::Client::new(),
        auth: format!("http://{address}"),
        key: "test".into(),
    };
    let mut ha = HeaderMap::new();
    ha.insert("authorization", "Bearer a".parse().unwrap());
    let mut hb = HeaderMap::new();
    hb.insert("authorization", "Bearer b".parse().unwrap());
    let result = start(
        State(s.clone()),
        ha.clone(),
        Json(Start {
            game: "memory".into(),
            params: json!({"pairs":6}),
        }),
    )
    .await
    .unwrap()
    .0;
    let id = result["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let seed = result["seed"].as_u64().unwrap() as u32;
    let mut rng = replay::Rng(seed);
    let mut cards: Vec<usize> = (0..6).chain(0..6).collect();
    for i in (1..12).rev() {
        let j = rng.index(i + 1);
        cards.swap(i, j)
    }
    let actions: Vec<Value> = (0..6)
        .flat_map(|v| {
            cards
                .iter()
                .enumerate()
                .filter(move |(_, c)| **c == v)
                .map(|(i, _)| json!(i))
        })
        .collect();
    assert_eq!(
        submit(
            State(s.clone()),
            hb,
            Path(id),
            Json(Submission {
                actions: actions.clone(),
                score: None
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        submit(
            State(s.clone()),
            ha.clone(),
            Path(id),
            Json(Submission {
                actions: actions.clone(),
                score: Some(999999)
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
    let first = submit(
        State(s.clone()),
        ha.clone(),
        Path(id),
        Json(Submission {
            actions: actions.clone(),
            score: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(first["score"], 999994);
    assert_eq!(
        first,
        submit(
            State(s.clone()),
            ha.clone(),
            Path(id),
            Json(Submission {
                actions,
                score: None
            })
        )
        .await
        .unwrap()
        .0
    );
    assert_eq!(
        submit(
            State(s.clone()),
            ha.clone(),
            Path(id),
            Json(Submission {
                actions: vec![json!(0)],
                score: None
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    for period in ["week", "month", "all"] {
        let board = leaderboard(
            State(s.clone()),
            ha.clone(),
            Query(Board {
                game: "memory".into(),
                period: period.into(),
                scope: "national".into(),
                school_id: None,
                pairs: None,
                level: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(board["ownBest"], 999994);
        assert_eq!(board["entries"], json!([]));
    }
    let _ = set_prefs(
        State(s.clone()),
        ha.clone(),
        Json(Pref {
            display_name: "Pseudonym".into(),
            participating: true,
        }),
    )
    .await
    .unwrap();
    let national_board = leaderboard(
        State(s.clone()),
        ha.clone(),
        Query(Board {
            game: "memory".into(),
            period: "all".into(),
            scope: "national".into(),
            school_id: None,
            pairs: None,
            level: None,
        }),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(national_board["entries"][0]["displayName"], "Pseudonym");
    let school_query = || {
        Query(Board {
            game: "memory".into(),
            period: "all".into(),
            scope: "school".into(),
            school_id: Some("test-school".into()),
            pairs: None,
            level: None,
        })
    };
    assert_eq!(
        leaderboard(State(s.clone()), ha.clone(), school_query())
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("insert into cbgpu_schools.memberships values($1,'test-school','ACTIVE',true,now()-interval '1 day',null)").bind(a).execute(&db).await.unwrap();
    let board = leaderboard(State(s.clone()), ha.clone(), school_query())
        .await
        .unwrap()
        .0;
    assert_eq!(board["entries"][0]["displayName"], "Pseudonym");
    assert!(board["entries"][0].get("user_id").is_none());
    sqlx::query("update cbgpu_schools.memberships set verified=false")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        leaderboard(State(s.clone()), ha.clone(), school_query())
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    sqlx::query(
        "update cbgpu_arcade.sessions set completed_at=now()-interval '2 years' where id=$1",
    )
    .bind(id)
    .execute(&db)
    .await
    .unwrap();
    for period in ["week", "month"] {
        let board = leaderboard(
            State(s.clone()),
            ha.clone(),
            Query(Board {
                game: "memory".into(),
                period: period.into(),
                scope: "national".into(),
                school_id: None,
                pairs: None,
                level: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(board["ownBest"].is_null());
    }
    let fresh = start(
        State(s.clone()),
        ha.clone(),
        Json(Start {
            game: "2048".into(),
            params: json!({}),
        }),
    )
    .await
    .unwrap()
    .0;
    let expired: Uuid = fresh["id"].as_str().unwrap().parse().unwrap();
    sqlx::query(
        "update cbgpu_arcade.sessions set expires_at=now()-interval '1 second' where id=$1",
    )
    .bind(expired)
    .execute(&db)
    .await
    .unwrap();
    assert_eq!(
        submit(
            State(s.clone()),
            ha.clone(),
            Path(expired),
            Json(Submission {
                actions: vec![json!("left")],
                score: None
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::GONE
    );
    sqlx::query("update auth.users set banned_until=now()+interval '1 hour' where id=$1")
        .bind(a)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        prefs(State(s.clone()), ha).await.unwrap_err().0,
        StatusCode::FORBIDDEN
    );
    server.abort();
}
