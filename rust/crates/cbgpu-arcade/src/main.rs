mod action_games;
mod replay;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;
#[derive(Clone)]
struct App {
    db: PgPool,
    http: reqwest::Client,
    auth: String,
    key: String,
}
type Error = (StatusCode, Json<Value>);
fn err(s: StatusCode, m: &str) -> Error {
    (s, Json(json!({"error":m})))
}
fn db_err(_: sqlx::Error) -> Error {
    err(StatusCode::SERVICE_UNAVAILABLE, "arcade_unavailable")
}
async fn user(s: &App, h: &HeaderMap) -> Result<Uuid, Error> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.starts_with("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "authentication_required"))?;
    let res = s
        .http
        .get(format!("{}/auth/v1/user", s.auth))
        .header("authorization", token)
        .header("apikey", &s.key)
        .send()
        .await
        .map_err(|_| {
            err(
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication_unavailable",
            )
        })?;
    if !res.status().is_success() {
        return Err(err(StatusCode::UNAUTHORIZED, "invalid_session"));
    }
    let v: Value = res
        .json()
        .await
        .map_err(|_| err(StatusCode::UNAUTHORIZED, "invalid_session"))?;
    let id: Uuid = v["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "invalid_session"))?;
    let active:bool=sqlx::query_scalar("select exists(select 1 from auth.users a join public.profiles p on p.id=a.id where a.id=$1 and a.deleted_at is null and (a.banned_until is null or a.banned_until<=now()) and p.account_status='ACTIVE')").bind(id).fetch_one(&s.db).await.map_err(db_err)?;
    if !active {
        return Err(err(StatusCode::FORBIDDEN, "inactive_account"));
    }
    Ok(id)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    game: String,
    #[serde(default)]
    params: Value,
}
async fn start(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Start>,
) -> Result<Json<Value>, Error> {
    let u = user(&s, &h).await?;
    if !["2048", "memory", "mines", "snake", "flappy", "tetris"].contains(&v.game.as_str()) {
        return Err(err(StatusCode::BAD_REQUEST, "unsupported_game"));
    }
    let params = match v.game.as_str() {
        "memory" => {
            let n = v.params["pairs"].as_u64().unwrap_or(6);
            if ![6, 8, 12].contains(&n) {
                return Err(err(StatusCode::BAD_REQUEST, "invalid_pairs"));
            };
            json!({"pairs":n})
        }
        "mines" => {
            let l = v.params["level"].as_str().unwrap_or("easy");
            if !["easy", "medium", "hard"].contains(&l) {
                return Err(err(StatusCode::BAD_REQUEST, "invalid_level"));
            };
            json!({"level":l})
        }
        _ => json!({}),
    };
    let id = Uuid::new_v4();
    let seed = (Uuid::new_v4().as_u128() as u32).max(1);
    let row=sqlx::query("insert into cbgpu_arcade.sessions(id,user_id,game,seed,params) select $1,$2,$3,$4,$5 where (select count(*) from cbgpu_arcade.sessions where user_id=$2 and created_at>now()-interval '1 hour')<120 returning expires_at::text").bind(id).bind(u).bind(&v.game).bind(i64::from(seed)).bind(&params).fetch_optional(&s.db).await.map_err(db_err)?.ok_or_else(||err(StatusCode::TOO_MANY_REQUESTS,"session_limit"))?;
    Ok(Json(
        json!({"id":id,"game":v.game,"seed":seed,"params":params,"rules_version":format!("{}-v1",v.game),"expires_at":row.get::<String,_>(0)}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Submission {
    actions: Vec<Value>,
    score: Option<i64>,
}
async fn submit(
    State(s): State<App>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(v): Json<Submission>,
) -> Result<Json<Value>, Error> {
    let u = user(&s, &h).await?;
    if v.actions.len() > 20000 {
        return Err(err(StatusCode::BAD_REQUEST, "too_many_actions"));
    }
    let hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&v.actions).unwrap())
    );
    let mut tx = s.db.begin().await.map_err(db_err)?;
    let r=sqlx::query("select game,seed,params,submission_hash,score,expires_at>now() as valid, (extract(epoch from (now()-created_at))*1000)::bigint as elapsed from cbgpu_arcade.sessions where id=$1 and user_id=$2 for update").bind(id).bind(u).fetch_optional(&mut *tx).await.map_err(db_err)?.ok_or_else(||err(StatusCode::NOT_FOUND,"session_not_found"))?;
    let old: Option<String> = r.get("submission_hash");
    let score = if let Some(old) = old {
        if old != hash {
            return Err(err(StatusCode::CONFLICT, "session_already_completed"));
        }
        r.get::<i64, _>("score")
    } else {
        if !r.get::<bool, _>("valid") {
            return Err(err(StatusCode::GONE, "session_expired"));
        }
        let game: String = r.get("game");
        let params: Value = r.get("params");
        if (r.get::<i64, _>("elapsed").max(0) as u64).saturating_add(2000)
            < action_games::min_duration_ms(&game, &v.actions)
        {
            return Err(err(StatusCode::BAD_REQUEST, "impossible_duration"));
        }
        let score = replay::replay(&game, r.get::<i64, _>("seed") as u32, &params, &v.actions)
            .map_err(|m| err(StatusCode::BAD_REQUEST, m))?;
        if v.score.is_some_and(|n| n != score) {
            return Err(err(StatusCode::BAD_REQUEST, "score_mismatch"));
        }
        sqlx::query("update cbgpu_arcade.sessions set completed_at=now(),score=$2,submission_hash=$3 where id=$1").bind(id).bind(score).bind(hash).execute(&mut *tx).await.map_err(db_err)?;
        score
    };
    if v.score.is_some_and(|n| n != score) {
        return Err(err(StatusCode::BAD_REQUEST, "score_mismatch"));
    }
    let best: Option<i64> = sqlx::query_scalar(
        "select max(score) from cbgpu_arcade.sessions where user_id=$1 and game=$2 and params=$3",
    )
    .bind(u)
    .bind(r.get::<String, _>("game"))
    .bind(r.get::<Value, _>("params"))
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Json(json!({"score":score,"personal_best":best})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pref {
    display_name: String,
    participating: bool,
}
async fn prefs(State(s): State<App>, h: HeaderMap) -> Result<Json<Value>, Error> {
    let u = user(&s, &h).await?;
    let r =
        sqlx::query("select display_name,participating from cbgpu_arcade.players where user_id=$1")
            .bind(u)
            .fetch_optional(&s.db)
            .await
            .map_err(db_err)?;
    Ok(Json(match r {
        Some(r) => json!({"display_name":r.get::<String,_>(0),"participating":r.get::<bool,_>(1)}),
        None => json!({"display_name":"Player","participating":false}),
    }))
}
async fn set_prefs(
    State(s): State<App>,
    h: HeaderMap,
    Json(v): Json<Pref>,
) -> Result<Json<Value>, Error> {
    let u = user(&s, &h).await?;
    let name = v.display_name.trim();
    if !(2..=24).contains(&name.chars().count())
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || " _-".contains(c))
    {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_pseudonym"));
    }
    sqlx::query("insert into cbgpu_arcade.players(user_id,display_name,participating) values($1,$2,$3) on conflict(user_id) do update set display_name=$2,participating=$3").bind(u).bind(name).bind(v.participating).execute(&s.db).await.map_err(db_err)?;
    Ok(Json(
        json!({"display_name":name,"participating":v.participating}),
    ))
}
#[derive(Deserialize)]
struct Board {
    game: String,
    #[serde(default = "all")]
    period: String,
    #[serde(default = "national")]
    scope: String,
    school_id: Option<String>,
    pairs: Option<u64>,
    level: Option<String>,
}
fn all() -> String {
    "all".into()
}
fn national() -> String {
    "national".into()
}
async fn leaderboard(
    State(s): State<App>,
    h: HeaderMap,
    Query(q): Query<Board>,
) -> Result<Json<Value>, Error> {
    let u = user(&s, &h).await?;
    if !["2048", "memory", "mines", "snake", "flappy", "tetris"].contains(&q.game.as_str())
        || !["week", "month", "all"].contains(&q.period.as_str())
        || !["national", "school"].contains(&q.scope.as_str())
    {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_filter"));
    }
    let school = if q.scope == "school" {
        let id = q
            .school_id
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "school_required"))?;
        let allowed:bool=sqlx::query_scalar("select exists(select 1 from cbgpu_schools.memberships where user_id=$1 and school_id=$2 and status='ACTIVE' and verified and starts_at<=now() and (ends_at is null or ends_at>now()))").bind(u).bind(&id).fetch_one(&s.db).await.map_err(db_err)?;
        if !allowed {
            return Err(err(
                StatusCode::FORBIDDEN,
                "verified_school_membership_required",
            ));
        }
        Some(id)
    } else {
        None
    };
    let params = match q.game.as_str() {
        "memory" => json!({"pairs":q.pairs.unwrap_or(6)}),
        "mines" => json!({"level":q.level.unwrap_or("easy".into())}),
        _ => json!({}),
    };
    let rows=sqlx::query("with best as (select x.user_id,max(x.score) as score from cbgpu_arcade.sessions x where x.game=$1 and x.params=$2 and x.completed_at is not null and ($3='all' or x.completed_at>=case when $3='week' then date_trunc('week',now() at time zone 'Asia/Seoul') at time zone 'Asia/Seoul' else date_trunc('month',now() at time zone 'Asia/Seoul') at time zone 'Asia/Seoul' end) and exists(select 1 from auth.users a join public.profiles ap on ap.id=a.id where a.id=x.user_id and a.deleted_at is null and (a.banned_until is null or a.banned_until<=now()) and ap.account_status='ACTIVE') and exists(select 1 from cbgpu_schools.memberships m where m.user_id=x.user_id and m.status='ACTIVE' and m.verified and m.starts_at<=now() and (m.ends_at is null or m.ends_at>now()) and ($4::text is null or m.school_id=$4)) group by x.user_id), ranked as (select b.user_id,b.score,p.display_name,rank() over(order by b.score desc) as rank from best b join cbgpu_arcade.players p on p.user_id=b.user_id and p.participating) select user_id,score,display_name,rank from ranked where rank<=100 or user_id=$5 order by rank,display_name").bind(&q.game).bind(&params).bind(&q.period).bind(&school).bind(u).fetch_all(&s.db).await.map_err(db_err)?;
    let own_rank = rows
        .iter()
        .find(|r| r.get::<Uuid, _>("user_id") == u)
        .map(|r| r.get::<i64, _>("rank"));
    let own_best:Option<i64>=sqlx::query_scalar("select max(score) from cbgpu_arcade.sessions where user_id=$1 and game=$2 and params=$3 and completed_at is not null and ($4='all' or completed_at>=case when $4='week' then date_trunc('week',now() at time zone 'Asia/Seoul') at time zone 'Asia/Seoul' else date_trunc('month',now() at time zone 'Asia/Seoul') at time zone 'Asia/Seoul' end)").bind(u).bind(&q.game).bind(params).bind(&q.period).fetch_one(&s.db).await.map_err(db_err)?;
    let school_name: Option<String> = if let Some(ref id) = school {
        sqlx::query_scalar("select coalesce(nullif(display_name,''),official_name) from cbgpu_schools.schools where id=$1").bind(id).fetch_optional(&s.db).await.map_err(db_err)?
    } else {
        None
    };
    let entries:Vec<Value>=rows.iter().filter(|r|r.get::<i64,_>("rank")<=100).take(100).map(|r|json!({"rank":r.get::<i64,_>("rank"),"displayName":r.get::<String,_>("display_name"),"score":r.get::<i64,_>("score")})).collect();
    Ok(Json(
        json!({"ownBest":own_best,"ownRank":own_rank,"entries":entries,"schoolName":school_name}),
    ))
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: std::net::SocketAddr = std::env::var("CBGPU_ARCADE_LISTEN")
        .unwrap_or("127.0.0.1:3552".into())
        .parse()?;
    if !addr.ip().is_loopback() {
        return Err("arcade must bind loopback".into());
    }
    let state = App {
        db: sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(&std::env::var("CBGPU_ARCADE_DATABASE_URL")?)
            .await?,
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()?,
        auth: std::env::var("CBGPU_ARCADE_AUTH_URL")?
            .trim_end_matches('/')
            .into(),
        key: std::env::var("CBGPU_ARCADE_ANON_KEY")?,
    };
    let app = Router::new()
        .route("/v1/arcade/sessions", post(start))
        .route("/v1/arcade/sessions/{id}/submit", post(submit))
        .route("/v1/arcade/player", get(prefs).put(set_prefs))
        .route("/v1/arcade/leaderboard", get(leaderboard))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(state);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
#[cfg(test)]
mod tests;
