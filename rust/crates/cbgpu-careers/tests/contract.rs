use axum::{body::Body, http::Request};
use cbgpu_careers::{App, issue_token, router, valid_token, validate};
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

fn fixture() -> App {
    App {
        pool: sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgresql://unused:unused@127.0.0.1:1/unused")
            .unwrap(),
        key: Arc::new(b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_vec()),
        roles: Arc::new(
            json!([{"id":"frontend","title":"Frontend & UI","name":"프론트엔드 개발"}]),
        ),
        public: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("careers/public"),
        origin: "https://careers.cbgpu.com".into(),
        release: "contract-test".into(),
    }
}
#[tokio::test]
async fn python_compatible_tokens_expire_and_cannot_be_forged() {
    let app = fixture();
    // Python uses UTF-8 bytes of the key's hexadecimal text, not decoded bytes.
    let output=std::process::Command::new("python3").args(["-c","import hmac,hashlib; s='1700000000.'+'a'*32; print(s+'.'+hmac.new(b'0123456789abcdef'*4,s.encode(),hashlib.sha256).hexdigest())"]).output().unwrap();
    let token = String::from_utf8(output.stdout).unwrap();
    let token = token.trim();
    assert!(valid_token(&app.key, token, 1700000000));
    assert!(valid_token(&app.key, token, 1700007199));
    assert!(!valid_token(&app.key, token, 1700007200));
    assert!(!valid_token(&app.key, token, 1699999999));
    assert!(!valid_token(b"other", token, 1700000000));
    let issued = issue_token(&app.key, 1700000000);
    assert!(valid_token(&app.key, &issued, 1700000000));
}

#[test]
fn resend_retry_body_is_byte_identical_to_the_original_sender() {
    let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../careers");
    let script = r#"
import sys,contextlib,os,json
sys.path.insert(0,sys.argv[1]); import server as s
class DB:
 def execute(self,*args): return self
 def commit(self): pass
 def fetchall(self): return [{'id':'CB-261116-A1B2C3D4','role':'frontend'}]
@contextlib.contextmanager
def connect(): yield DB()
class Reply:
 def __enter__(self): return self
 def __exit__(self,*args): pass
 def read(self): return b'{"id":"synthetic-provider-ack"}'
def send(request,timeout):
 sys.stdout.buffer.write(request.data)
 return Reply()
s.connect=connect;s.urllib.request.urlopen=send
os.environ.pop('CAREERS_MAIL_ENV',None);os.environ['RESEND_API_KEY']='re_test_only'
s.process_outbox()
"#;
    let output = std::process::Command::new("python3")
        .args(["-c", script, source.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        cbgpu_careers::outbox::mail_body("프론트엔드 개발", "CB-261116-A1B2C3D4").as_bytes()
    );
}
#[tokio::test]
async fn validates_unicode_lengths_consent_honeypot_and_private_urls() {
    let app = fixture();
    let mut data = json!({"role":"frontend","name":"학생","email":"test@example.com","motivation":"가".repeat(20),"consent":true,"token":issue_token(&app.key,1700000000)});
    assert!(validate(&app, &data, 1700000000).is_ok());
    data["name"] = json!("가".repeat(60));
    assert!(validate(&app, &data, 1700000000).is_ok());
    data["name"] = json!("가".repeat(61));
    assert!(validate(&app, &data, 1700000000).is_err());
    data["name"] = json!("학생");
    data["portfolio"] = json!("https://user:pass@example.com");
    assert!(validate(&app, &data, 1700000000).is_err());
    data["portfolio"] = json!("https://github.com/example");
    data["consent"] = json!(false);
    assert!(validate(&app, &data, 1700000000).is_err());
    data["consent"] = json!(true);
    data["website"] = json!({"bot":true});
    assert!(validate(&app, &data, 1700000000).is_err());
}
#[tokio::test]
async fn static_pages_stay_identical_and_public_routes_never_list_applicants() {
    let app = fixture();
    let expected = std::fs::read(app.public.join("onboarding.html")).unwrap();
    let service = router(app);
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .uri("/onboarding")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-frame-options"], "DENY");
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .as_ref(),
        expected
    );
    for path in [
        "/api/applications",
        "/applications.sqlite",
        "/../form.key",
        "/apply/nonexistent",
    ] {
        assert_eq!(
            service
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            404
        );
    }
    let request = Request::builder()
        .method("POST")
        .uri("/api/applications")
        .header("origin", "https://evil.example")
        .header("content-type", "application/json")
        .header("content-length", "2")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(service.oneshot(request).await.unwrap().status(), 403);
}
