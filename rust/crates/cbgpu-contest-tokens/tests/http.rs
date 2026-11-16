use axum::{body::Body, http::Request};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cbgpu_contest_tokens::{
    Identity, MintRequest, Persistence, RegistrationInput, Result, TokenHash, Verified,
    api::{Api, router},
    fail, generate,
};
use cbgpu_contests_api::Principal;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tower::ServiceExt;
const KEY: &str = "candidate-private-key-with32bytes-minimum";
const OWNER: &str = "00000000-0000-4000-8000-000000000002";
const CONTEST: &str = "60000000-0000-4000-8000-000000000001";
const ID: &str = "90000000-0000-4000-8000-000000000001";
fn jwt() -> String {
    format!("e30.{}.fixture",URL_SAFE_NO_PAD.encode(json!({"sub":OWNER,"session_id":"10000000-0000-4000-8000-000000000002","role":"authenticated","exp":9999999999u64}).to_string()))
}
struct Auth {
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl Identity for Auth {
    async fn authenticate(&self, token: &str) -> Result<Principal> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if token != jwt() {
            return Err(fail(401, "unauthorized", "Fixture rejects unverified JWT"));
        }
        Ok(Principal {
            subject: OWNER.parse().unwrap(),
            may_register: true,
        })
    }
}
struct Fake {
    hash: Mutex<Option<Vec<u8>>>,
    active: AtomicBool,
    writes: AtomicUsize,
}
#[async_trait::async_trait]
impl Persistence for Fake {
    async fn mint(&self, actor: &Verified, hash: &TokenHash, input: &MintRequest) -> Result<Value> {
        assert_eq!(actor.subject.to_string(), OWNER);
        assert_eq!(input.expires_in_days, 30);
        *self.hash.lock().unwrap() = Some(hash.bytes().to_vec());
        Ok(
            json!({"id":ID,"name":input.name,"scopes":input.scopes,"allowed_contest_ids":input.allowed_contest_ids}),
        )
    }
    async fn list(&self, actor: &Verified) -> Result<Value> {
        assert_eq!(actor.subject.to_string(), OWNER);
        Ok(json!({"tokens":[{"id":ID,"name":"Fixture"}]}))
    }
    async fn revoke(&self, actor: &Verified, id: uuid::Uuid) -> Result<bool> {
        assert_eq!(actor.subject.to_string(), OWNER);
        assert_eq!(id.to_string(), ID);
        self.active.store(false, Ordering::SeqCst);
        Ok(true)
    }
    async fn read(&self, _: &TokenHash, id: Option<uuid::Uuid>, own: bool) -> Result<Value> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(fail(401, "invalid_contest_token", "Revoked"));
        }
        if own {
            return Ok(json!({"registration":{"user_id":OWNER,"contest_id":CONTEST}}));
        }
        Ok(if id.is_some() {
            json!({"contest":{"id":CONTEST,"title":"Fixture","registration_questions":[]}})
        } else {
            json!({"contests":[]})
        })
    }
    async fn register(
        &self,
        _: &TokenHash,
        id: uuid::Uuid,
        input: &RegistrationInput,
    ) -> Result<Value> {
        assert_eq!(id.to_string(), CONTEST);
        assert_eq!(input.attendee_name, "Fixture");
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"registration":{"user_id":OWNER,"contest_id":CONTEST,"status":"confirmed"}}))
    }
    async fn cancel(&self, _: &TokenHash, id: uuid::Uuid) -> Result<Value> {
        assert_eq!(id.to_string(), CONTEST);
        Ok(json!({"registration":{"user_id":OWNER,"status":"cancelled"}}))
    }
}
fn fixture() -> (axum::Router, Arc<Fake>, Arc<Auth>) {
    let auth = Arc::new(Auth {
        calls: AtomicUsize::new(0),
    });
    let store = Arc::new(Fake {
        hash: Mutex::new(None),
        active: AtomicBool::new(true),
        writes: AtomicUsize::new(0),
    });
    let api = Api::new(auth.clone(), store.clone(), KEY.as_bytes().to_vec()).unwrap();
    (router(api), store, auth)
}
fn request(method: &str, path: &str, token: &str, value: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("x-cbgpu-contest-key", KEY)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(if value.is_null() {
            Body::empty()
        } else {
            Body::from(value.to_string())
        })
        .unwrap()
}
fn mint() -> Value {
    json!({"name":"Fixture","scopes":["contests:read","contests:register"],"allowed_contest_ids":[CONTEST]})
}
#[tokio::test]
async fn mint_requires_real_jwt_and_returns_random_secret_once_hash_only_to_persistence() {
    let (app, store, auth) = fixture();
    let (pat, _) = generate();
    assert_eq!(
        app.clone()
            .oneshot(request("POST", "/v1/tokens", &pat, mint()))
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(auth.calls.load(Ordering::SeqCst), 0);
    let response = app
        .clone()
        .oneshot(request("POST", "/v1/tokens", &jwt(), mint()))
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let value: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let secret = value["token"].as_str().unwrap();
    assert_eq!(secret.len(), 52);
    assert_eq!(
        TokenHash::parse(secret).unwrap().bytes(),
        store.hash.lock().unwrap().as_ref().unwrap()
    );
    assert!(value["record"].get("token_sha256").is_none());
    let listed = app
        .oneshot(request("GET", "/v1/tokens", &jwt(), Value::Null))
        .await
        .unwrap()
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert!(!String::from_utf8_lossy(&listed).contains(secret));
}
#[tokio::test]
async fn strict_lifecycle_fields_do_not_accept_owner_identity_scope_or_expiration_overrides() {
    let (app, _, _) = fixture();
    for (field, value) in [
        ("owner", json!(OWNER)),
        ("user_id", json!(OWNER)),
        ("token_sha256", json!("chosen")),
        ("status", json!("admin")),
        ("expires_at", json!("never")),
    ] {
        let mut input = mint();
        input[field] = value;
        assert_eq!(
            app.clone()
                .oneshot(request("POST", "/v1/tokens", &jwt(), input))
                .await
                .unwrap()
                .status(),
            400
        )
    }
    for value in [json!(["admin"]), json!([])] {
        let mut input = mint();
        input["scopes"] = value;
        assert_eq!(
            app.clone()
                .oneshot(request("POST", "/v1/tokens", &jwt(), input))
                .await
                .unwrap()
                .status(),
            400
        )
    }
    let mut input = mint();
    input["expires_in_days"] = json!(91);
    assert_eq!(
        app.oneshot(request("POST", "/v1/tokens", &jwt(), input))
            .await
            .unwrap()
            .status(),
        400
    );
}
#[tokio::test]
async fn delegated_actions_have_no_identity_override_and_revocation_is_immediate() {
    let (app, store, _) = fixture();
    let (pat, _) = generate();
    let input = json!({"attendee_name":"Fixture","answers":{}});
    for path in [
        format!("/v1/contests/{CONTEST}/registrations"),
        "/v1/contests/register".into(),
    ] {
        let mut v = input.clone();
        if path.ends_with("register") {
            v["contest_id"] = json!(CONTEST)
        }
        assert_eq!(
            app.clone()
                .oneshot(request("POST", &path, &pat, v))
                .await
                .unwrap()
                .status(),
            201
        )
    }
    assert_eq!(store.writes.load(Ordering::SeqCst), 2);
    let mut spoof = input;
    spoof["user_id"] = json!(OWNER);
    assert_eq!(
        app.clone()
            .oneshot(request(
                "POST",
                &format!("/v1/contests/{CONTEST}/registrations"),
                &pat,
                spoof
            ))
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        app.clone()
            .oneshot(request(
                "GET",
                &format!("/v1/contests/{CONTEST}/registrations/me?user_id={OWNER}"),
                &pat,
                Value::Null
            ))
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        app.clone()
            .oneshot(request(
                "DELETE",
                &format!("/v1/tokens/{ID}"),
                &jwt(),
                Value::Null
            ))
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        app.oneshot(request("GET", "/v1/contests", &pat, Value::Null))
            .await
            .unwrap()
            .status(),
        401
    );
}
#[tokio::test]
async fn ai_credentials_cookies_missing_origin_key_and_owner_headers_never_authorize() {
    let (app, _, _) = fixture();
    assert_eq!(
        app.clone()
            .oneshot(request(
                "GET",
                "/v1/contests",
                "cbgpu_ai_private",
                Value::Null
            ))
            .await
            .unwrap()
            .status(),
        401
    );
    let (pat, _) = generate();
    for header in ["x-cbgpu-owner", "origin"] {
        let mut r = request("GET", "/v1/contests", &pat, Value::Null);
        r.headers_mut().insert(header, OWNER.parse().unwrap());
        assert!(
            app.clone()
                .oneshot(r)
                .await
                .unwrap()
                .status()
                .is_client_error()
        )
    }
    let mut r = request("GET", "/v1/tokens", &jwt(), Value::Null);
    r.headers_mut().remove("authorization");
    r.headers_mut()
        .insert("cookie", "sb-cbgpu-auth-token.0=ignored".parse().unwrap());
    assert_eq!(app.clone().oneshot(r).await.unwrap().status(), 401);
    let mut r = request("GET", "/v1/contests", &pat, Value::Null);
    r.headers_mut().remove("x-cbgpu-contest-key");
    assert_eq!(app.oneshot(r).await.unwrap().status(), 403);
}
#[test]
fn token_randomness_shape_and_scope_defaults_are_explicit() {
    let mut hashes = std::collections::BTreeSet::new();
    for _ in 0..128 {
        let (secret, hash) = generate();
        assert_eq!(TokenHash::parse(&secret).unwrap().bytes(), hash.bytes());
        assert!(hashes.insert(hash.bytes().to_vec()));
    }
    assert!(TokenHash::parse("cbgpu_ct_short").is_err());
    let input: MintRequest = serde_json::from_value(mint()).unwrap();
    assert_eq!(input.expires_in_days, 30);
}
