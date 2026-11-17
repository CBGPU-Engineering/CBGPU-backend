//! Disposable network-none PostgreSQL only; synthetic report values are never production data.
use cbgpu_ad_revenue::report::{Input, prepare};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};
const ADMIN: &str = "ae010000-0000-4000-8000-000000000001";
const SID: &str = "ae020000-0000-4000-8000-000000000001";
struct Db {
    name: String,
}
impl Drop for Db {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.name])
            .output();
    }
}
impl Db {
    fn sql(&self, text: &str) -> Result<String, String> {
        let mut child = Command::new("docker")
            .args([
                "exec",
                "-i",
                &self.name,
                "psql",
                "-U",
                "postgres",
                "-v",
                "ON_ERROR_STOP=1",
                "-qAt",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        if out.status.success() {
            Ok(String::from_utf8(out.stdout).unwrap())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into())
        }
    }
    fn call(&self, action: &str, payload: &Value) -> Result<Value, String> {
        let claims = json!({"role":"authenticated","sub":ADMIN,"session_id":SID});
        let text = format!(
            "begin;set local role authenticated;set local request.jwt.claims='{}';select public.cbgpu_ad_revenue_api('{}','{}');commit;",
            claims,
            action,
            payload.to_string().replace('\'', "''")
        );
        self.sql(&text)
            .and_then(|s| serde_json::from_str(s.trim()).map_err(|e| e.to_string()))
    }
}
fn report(day: &str, amount: &str, kind: &str) -> Value {
    let input:Input=serde_json::from_value(json!({"csv":format!("Date,Unit,Revenue,Impressions,Clicks,Requests\n{day},DAN-kVpBmlPRbOvGb6oi,{amount},100,2,200\n"),"granularity":"DAY","period_start":"2026-10-01","period_end":"2026-10-31","currency":"KRW","kind":kind,"timezone":"Asia/Seoul","columns":{"date":"Date","unit_id":"Unit","revenue":"Revenue","impressions":"Impressions","clicks":"Clicks","requests":"Requests"},"format":{"delimiter":",","date_format":"YYYY-MM-DD","decimal_separator":".","group_separator":","}})).unwrap();
    let mut p = serde_json::to_value(prepare(input).unwrap()).unwrap();
    p["confirmed"] = json!(true);
    p
}
#[test]
#[ignore = "Requires disposable Docker PostgreSQL17; no production credentials"]
fn actual_import_atomic_overlap_idempotency_concurrency_privacy_and_experiments() {
    let db = Db {
        name: format!("cbgpu-ad-report-test-{}", std::process::id()),
    };
    let out = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &db.name,
            "--network",
            "none",
            "--memory",
            "384m",
            "--cpus",
            "1",
            "--tmpfs",
            "/var/lib/postgresql/data:rw,size=256m",
            "-e",
            "POSTGRES_HOST_AUTH_METHOD=trust",
            "postgres:17-alpine",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut ready = false;
    for _ in 0..100 {
        let out = Command::new("docker")
            .args(["logs", &db.name])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&out.stdout).contains("PostgreSQL init process complete")
            && db.sql("select 1").is_ok()
        {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(ready);
    db.sql(&format!("create role anon;create role authenticated;create role service_role;create schema auth;create table auth.users(id uuid primary key,email_confirmed_at timestamptz,deleted_at timestamptz,banned_until timestamptz);create table auth.sessions(id uuid primary key,user_id uuid,not_after timestamptz);create table public.profiles(id uuid,is_admin bool,account_status text);create function auth.jwt() returns jsonb language sql as $$select coalesce(nullif(current_setting('request.jwt.claims',true),'')::jsonb,'{{}}'::jsonb)$$;create function auth.uid() returns uuid language sql as $$select(auth.jwt()->>'sub')::uuid$$;create function auth.role() returns text language sql as $$select auth.jwt()->>'role'$$;grant usage on schema auth to authenticated;grant execute on all functions in schema auth to authenticated;insert into auth.users values('{ADMIN}',now(),null,null);insert into auth.sessions values('{SID}','{ADMIN}',null);insert into public.profiles values('{ADMIN}',true,'ACTIVE');")).unwrap();
    let schema = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .join("supabase/migrations/20261117103045_official_ad_revenue_reporting.sql");
    let source = std::fs::read_to_string(&schema).unwrap();
    db.sql(&source).unwrap();
    db.sql(&source).unwrap();
    let attribution = std::fs::read_to_string(
        schema
            .parent()
            .unwrap()
            .join("20261117113827_ad_revenue_unit_format_attribution.sql"),
    )
    .unwrap();
    db.sql(&attribution).unwrap();
    db.sql(&attribution).unwrap();
    let p = report("2026-10-01", "1.25", "ESTIMATED");
    let preview = db.call("preview", &p).unwrap();
    assert_eq!(preview["device_basis"], "AD_UNIT_FORMAT");
    assert!(preview["physical_device_revenue"].is_null());
    assert_eq!(
        preview["devices"][0]["physical_device_reason"],
        "NO_OFFICIAL_DEVICE_DIMENSION"
    );
    assert_eq!(preview["devices"][0]["revenue_krw"], "1.25");
    assert_eq!(preview["devices"][0]["ecpm_krw"], "12.5");
    assert!(preview["devices"][0]["rpuv_krw"].is_null());
    assert!(preview["devices"][0]["fill_rate_percent"].is_null());
    assert_eq!(db.call("import", &p).unwrap()["duplicate"], false);
    assert_eq!(db.call("import", &p).unwrap()["duplicate"], true);
    let bad = report("2026-10-01", "2.5", "ESTIMATED");
    assert!(
        db.call("import", &bad)
            .unwrap_err()
            .contains("REPORT_PERIOD_OVERLAP")
    );
    assert_eq!(
        db.sql("select count(*) from cbgpu_ad_reports.facts")
            .unwrap()
            .trim(),
        "1"
    );
    let confirmed = report("2026-10-01", "80.5", "CONFIRMED");
    db.call("import", &confirmed).unwrap();
    let a = db
        .call(
            "analytics",
            &json!({"from":"2026-10-01","to":"2026-10-31","kind":"ESTIMATED"}),
        )
        .unwrap();
    assert_eq!(a["devices"][0]["revenue_krw"], "1.25");
    assert_eq!(a["daily"].as_array().unwrap().len(), 1);
    let a = db
        .call(
            "analytics",
            &json!({"from":"2026-10-01","to":"2026-10-31","kind":"CONFIRMED"}),
        )
        .unwrap();
    assert_eq!(a["devices"][0]["revenue_krw"], "80.5");
    let concurrent = report("2026-10-02", "0.000001", "ESTIMATED");
    std::thread::scope(|scope| {
        let a = scope.spawn(|| db.call("import", &concurrent));
        let b = scope.spawn(|| db.call("import", &concurrent));
        for result in [a.join().unwrap(), b.join().unwrap()] {
            if let Err(error) = result {
                assert!(error.contains("REPORT_IMPORT_BUSY"));
            }
        }
    });
    assert_eq!(db.call("import", &concurrent).unwrap()["duplicate"], true);
    assert_eq!(
        db.sql("select count(*) from cbgpu_ad_reports.facts where kind='ESTIMATED'")
            .unwrap()
            .trim(),
        "2"
    );
    let ex = json!({"name":"Manual console plan","device":"DESKTOP","unit_id":"DAN-kVpBmlPRbOvGb6oi","period_start":"2026-10-01","period_end":"2026-10-31","timezone":"Asia/Seoul","interval_seconds":60,"console_verified":false,"notes":"No SDK timer"});
    let created = db.call("experiment_create", &ex).unwrap()["experiment"].clone();
    assert_eq!(created["provider_verified"], false);
    assert_eq!(created["phase"], "PLANNED");
    let mut edit = ex;
    edit["id"] = created["id"].clone();
    edit["expected_revision"] = json!(1);
    edit["console_verified"] = json!(true);
    assert_eq!(
        db.call("experiment_update", &edit).unwrap()["experiment"]["phase"],
        "MANUALLY_VERIFIED"
    );
    assert!(
        db.call("experiment_update", &edit)
            .unwrap_err()
            .contains("EXPERIMENT_REVISION_CONFLICT")
    );
    db.sql("set role authenticated;select * from cbgpu_ad_reports.imports;")
        .unwrap_err();
    db.sql("reset role;").unwrap();
    db.sql(&format!(
        "update public.profiles set is_admin=false where id='{ADMIN}'"
    ))
    .unwrap();
    assert!(
        db.call("analytics", &json!({"from":"2026-10-01","to":"2026-10-31"}))
            .unwrap_err()
            .contains("FRESH_PLATFORM_ADMIN_REQUIRED")
    );
    db.sql(&format!("update public.profiles set is_admin=true where id='{ADMIN}';delete from auth.sessions where id='{SID}';")).unwrap();
    assert!(
        db.call("imports", &json!({}))
            .unwrap_err()
            .contains("FRESH_PLATFORM_ADMIN_REQUIRED")
    );
}
