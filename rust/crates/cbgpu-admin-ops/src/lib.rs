//! Explicit operator checks. No command changes production merely by being planned.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cbgpu_delivery::{
    agent::{Runtime, SystemRuntime},
    atomic,
};
use hmac::{Hmac, Mac};
use rsa::{
    RsaPrivateKey,
    pkcs1::DecodeRsaPrivateKey,
    pkcs1v15::SigningKey,
    pkcs8::DecodePrivateKey,
    signature::{SignatureEncoding, Signer},
    traits::PublicKeyParts,
};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{collections::BTreeMap, fs, path::Path, time::Duration};
pub type Result<T> = std::result::Result<T, String>;
pub struct Response {
    pub status: u16,
    pub body: Value,
}
pub trait Operator {
    fn run(
        &mut self,
        args: &[String],
        cwd: Option<&Path>,
        env: Option<&BTreeMap<String, String>>,
        input: Option<&[u8]>,
        timeout: u64,
    ) -> Result<String>;
    fn http(
        &mut self,
        url: &str,
        method: &str,
        headers: &BTreeMap<String, String>,
        body: Option<Vec<u8>>,
    ) -> Result<Response>;
    fn build(&mut self, directory: &Path, env: &BTreeMap<String, String>, log: &Path)
    -> Result<()>;
    fn wait(&mut self, duration: Duration);
    fn is_root(&self) -> bool;
}
pub struct SystemOperator;
impl Operator for SystemOperator {
    fn run(
        &mut self,
        args: &[String],
        cwd: Option<&Path>,
        env: Option<&BTreeMap<String, String>>,
        input: Option<&[u8]>,
        timeout: u64,
    ) -> Result<String> {
        SystemRuntime.execute(args, cwd, env, input, timeout)
    }
    fn http(
        &mut self,
        url: &str,
        method: &str,
        headers: &BTreeMap<String, String>,
        body: Option<Vec<u8>>,
    ) -> Result<Response> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Cannot create operator HTTP client")?;
        let mut request = client.request(
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| "Invalid HTTP method")?,
            url,
        );
        for (key, value) in headers {
            request = request.header(key, value);
        }
        if let Some(body) = body {
            request = request.body(body)
        }
        let response = request
            .send()
            .map_err(|_| "Operator HTTP request failed; credentials omitted")?;
        let status = response.status().as_u16();
        let body = response
            .json()
            .map_err(|_| "Operator endpoint returned invalid JSON")?;
        Ok(Response { status, body })
    }
    fn build(
        &mut self,
        directory: &Path,
        env: &BTreeMap<String, String>,
        log: &Path,
    ) -> Result<()> {
        SystemRuntime.build(&strings(&["npm", "run", "build"]), directory, env, log)
    }
    fn wait(&mut self, duration: Duration) {
        std::thread::sleep(duration)
    }
    fn is_root(&self) -> bool {
        unsafe { libc::geteuid() == 0 }
    }
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).into()).collect()
}
pub fn env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let source = fs::read_to_string(path).map_err(|_| "Private environment unavailable")?;
    let mut values = BTreeMap::new();
    for line in source.lines() {
        if line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim();
            let value = if value.len() > 1
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            values.insert(key.into(), value.into());
        }
    }
    Ok(values)
}
fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).map_err(|_| "Private JSON unavailable")?)
        .map_err(|_| "Invalid private JSON".into())
}
fn required<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    env.get(key)
        .filter(|v| !v.is_empty())
        .map(String::as_str)
        .ok_or_else(|| format!("Missing required private setting {key}"))
}
pub fn plan(command: &str) -> Result<Value> {
    let (retired, effect) = match command {
        "activate-local-backend" | "final-sync" | "configure-scheduled-jobs" => (
            true,
            "Historical source-cloud cutover completed. This command is permanently retired; it cannot truncate accounts, replace keys or replay obsolete source data.",
        ),
        "check-email-domain" => (
            false,
            "Read DNS and optional Resend domain status; write only private aggregate status.",
        ),
        "build-frontend" => (
            false,
            "Prepare private frontend environment and build selected checkout; no service restart or deployment.",
        ),
        "verify-backend" => (
            false,
            "Read Auth/RLS/profile/function health. Does not generate recovery links for real users.",
        ),
        "verify-push-credentials" => (
            false,
            "Exchange Firebase service JWT for an access token; no notification sent.",
        ),
        "verify-recovery-simulator" => (
            false,
            "Create only delivered@resend.dev simulator account, send simulator recovery, verify state, remove test account. Requires --allow-test-email.",
        ),
        "restart-controller-preserving-gpus" => (
            false,
            "Requires root and --allow-controller-restart; refuses while approved GPU reservations are active, preserves GPU containers and marker cleanup.",
        ),
        "verify-server-runtimes" => (
            false,
            "Use network-isolated root-path fixtures and a labeled disposable Java21/Maven Spring test container; no production container or volume touched.",
        ),
        _ => return Err("Unknown admin operation".into()),
    };
    Ok(
        json!({"command":command,"mode":"plan","retired":retired,"effect":effect,"requires_apply":!retired}),
    )
}
pub fn parse_txt(line: &str) -> Result<String> {
    let mut text = String::new();
    let (mut quoted, mut escaped) = (false, false);
    for c in line.chars() {
        if escaped {
            text.push(c);
            escaped = false
        } else if c == '\\' {
            escaped = true
        } else if c == '"' {
            quoted = !quoted
        } else if quoted || !c.is_whitespace() {
            text.push(c)
        }
    }
    if quoted || escaped {
        return Err("Malformed DNS TXT response".into());
    }
    Ok(text)
}
const DKIM: &str = "p=MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCo8NfPHtaXx7WxDyCt/cKPRDCKHxC1PtsHbFrgH+BTeQqdARVXiOAtUEsm8fw9Wk4waq9GN5j9gLiE2A8FMWhgVbzLqhCeu6Sf/XOcsWX1UpMacoKmAFQks3Gti4orfnbWkFDma646w7ftyERpN+oBgEx5gTwY7O2QljPMcQ0B7QIDAQAB";
pub fn check_email_domain(root: &Path, io: &mut impl Operator) -> Result<Value> {
    let mut records = Vec::new();
    for (name, kind, expected) in [
        ("resend._domainkey.cbgpu.com", "TXT", DKIM),
        ("rsend.cbgpu.com", "CNAME", "rsend-apne1.forge.rmta.net"),
        ("send.cbgpu.com", "CNAME", "send.forge.rmta.net"),
        ("_dmarc.cbgpu.com", "TXT", "v=DMARC1; p=none;"),
    ] {
        let output = io.run(
            &strings(&["dig", "+short", kind, name]),
            None,
            None,
            None,
            10,
        )?;
        let observed: Vec<String> = output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                if kind == "TXT" {
                    parse_txt(l)
                } else {
                    Ok(l.trim_end_matches('.').into())
                }
            })
            .collect::<Result<_>>()?;
        records.push(json!({"name":name,"type":kind,"matches":observed.iter().any(|v|v==expected),"observed":observed,"expected":expected}));
    }
    let mut status = json!({"checked_at":chrono::Utc::now().to_rfc3339(),"dns_ready":records.iter().all(|r|r["matches"]==true),"records":records,"resend_status":"not_accessible_with_sending_only_key"});
    let key_path = root.join("private/resend-management-key");
    if key_path.exists() {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&key_path)
            .map_err(|_| "Cannot inspect management key")?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err("Resend management key requires permissions600".into());
        }
        let key = fs::read_to_string(key_path).map_err(|_| "Cannot read management key")?;
        let response = io.http(
            "https://api.resend.com/domains",
            "GET",
            &BTreeMap::from([("Authorization".into(), format!("Bearer {}", key.trim()))]),
            None,
        )?;
        status["resend_status"] = if response.status == 200 {
            response.body["data"]
                .as_array()
                .and_then(|rows| rows.iter().find(|r| r["name"] == "cbgpu.com"))
                .map(|r| r["status"].clone())
                .unwrap_or(json!("not_in_this_account"))
        } else {
            json!(format!("unavailable_http_{}", response.status))
        };
    }
    atomic(
        &root.join("private/email-domain-status.json"),
        &serde_json::to_vec_pretty(&status).map_err(|_| "Cannot encode email status")?,
    )?;
    Ok(
        json!({"checked_at":status["checked_at"],"dns_ready":status["dns_ready"],"resend_status":status["resend_status"]}),
    )
}
pub fn frontend_environment(root: &Path) -> Result<BTreeMap<String, String>> {
    let env = env_file(&root.join(".env"))?;
    let secrets = read_json(&root.join("private/source-function-secrets.json"))?;
    let portal = read_json(&root.join("private/developer-portal.json"))?;
    let sender = read_json(&root.join("private/email-sender-status.json"))?;
    let value = |object: &Value, key: &str| {
        object[key]
            .as_str()
            .filter(|v| !v.contains(['\n', '\r']))
            .map(str::to_owned)
            .ok_or_else(|| format!("Missing private frontend setting {key}"))
    };
    let mut result = BTreeMap::from([
        (
            "CBGPU_EMAIL_FROM".into(),
            format!("CBGPU <{}>", value(&sender, "active")?),
        ),
        (
            "CBGPU_TRAFFIC_SECRET".into(),
            value(&portal, "ingestSecret")?,
        ),
        (
            "NEXT_PUBLIC_SUPABASE_URL".into(),
            "https://cbgpu.com/backend".into(),
        ),
        (
            "NEXT_PUBLIC_SUPABASE_ANON_KEY".into(),
            required(&env, "SUPABASE_PUBLISHABLE_KEY")?.into(),
        ),
        (
            "SUPABASE_SERVICE_ROLE_KEY".into(),
            required(&env, "SERVICE_ROLE_KEY")?.into(),
        ),
        (
            "EDUSET_TOKEN_SECRET".into(),
            value(&secrets, "EDUSET_TOKEN_SECRET")?,
        ),
        ("RESEND_API_KEY".into(), value(&secrets, "RESEND_API_KEY")?),
    ]);
    if result.values().any(|v| v.contains(['\n', '\r'])) {
        return Err("Invalid multiline frontend setting".into());
    }
    result.insert("NEXT_BUILD_DIR".into(), ".next-email-ops".into());
    Ok(result)
}
pub fn build_frontend(root: &Path, frontend: &Path, io: &mut impl Operator) -> Result<Value> {
    if !frontend.is_absolute() || !frontend.join("package.json").is_file() {
        return Err("Select an absolute frontend checkout with package.json".into());
    }
    let config = frontend_environment(root)?;
    let text = config
        .iter()
        .filter(|(k, _)| k.as_str() != "NEXT_BUILD_DIR")
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect::<String>();
    atomic(&root.join("private/frontend-runtime.env"), text.as_bytes())?;
    let mut env: BTreeMap<_, _> = std::env::vars().collect();
    env.extend(config);
    io.build(frontend, &env, &root.join("private/frontend-build.log"))?;
    Ok(json!({"ok":true,"built":true,"deployed":false}))
}
pub fn jwt_hs256(claims: &Value, secret: &str) -> Result<String> {
    let unsigned = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).map_err(|_| "Cannot encode JWT claims")?)
    );
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| "Invalid JWT key")?;
    mac.update(unsigned.as_bytes());
    Ok(format!(
        "{unsigned}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}
fn sql(io: &mut impl Operator, query: &str, user: &str) -> Result<String> {
    io.run(
        &strings(&[
            "docker",
            "exec",
            "-i",
            "cbgpu-supabase-db",
            "psql",
            "-U",
            user,
            "-d",
            "postgres",
            "-At",
            "-v",
            "ON_ERROR_STOP=1",
            "-f",
            "-",
        ]),
        None,
        None,
        Some(query.as_bytes()),
        30,
    )
}
fn local_api(
    io: &mut impl Operator,
    env: &BTreeMap<String, String>,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> Result<Response> {
    let mut headers = BTreeMap::from([
        (
            "apikey".into(),
            env.get("SUPABASE_PUBLISHABLE_KEY")
                .or_else(|| env.get("ANON_KEY"))
                .ok_or("Missing public API key")?
                .clone(),
        ),
        (
            "Authorization".into(),
            format!("Bearer {}", token.unwrap_or(required(env, "ANON_KEY")?)),
        ),
    ]);
    let method = if body.is_some() { "POST" } else { "GET" };
    let bytes = body
        .map(|v| serde_json::to_vec(&v))
        .transpose()
        .map_err(|_| "Cannot encode local request")?;
    if bytes.is_some() {
        headers.insert("Content-Type".into(), "application/json".into());
    }
    io.http(
        &format!("http://127.0.0.1:54321{path}"),
        method,
        &headers,
        bytes,
    )
}
pub fn verify_backend(root: &Path, io: &mut impl Operator) -> Result<Value> {
    let env = env_file(&root.join(".env"))?;
    let health = local_api(io, &env, "/auth/v1/health", None, None)?;
    if health.status != 200 {
        return Err("Auth health check failed".into());
    }
    for path in [
        "/rest/v1/profiles?select=id",
        "/rest/v1/gpu_ssh_keys?select=*",
    ] {
        let response = local_api(io, &env, path, None, None)?;
        if response.status != 200 || response.body != json!([]) {
            return Err("Anonymous RLS boundary failed; data omitted".into());
        }
    }
    let selected = sql(
        io,
        "select json_build_object('id',p.id,'email',u.email) from public.profiles p join auth.users u on u.id=p.id where coalesce(p.is_admin,false)=false and p.is_approved is distinct from false and u.deleted_at is null and (u.banned_until is null or u.banned_until<=clock_timestamp()) limit 1;",
        "postgres",
    )?;
    let user: Value = serde_json::from_str(selected.trim())
        .map_err(|_| "No approved non-admin fixture account available")?;
    let id = user["id"]
        .as_str()
        .filter(|v| uuid::Uuid::parse_str(v).is_ok())
        .ok_or("Invalid account identifier")?;
    let now = chrono::Utc::now().timestamp();
    let token = jwt_hs256(
        &json!({"aud":"authenticated","role":"authenticated","sub":id,"email":user["email"],"iss":required(&env,"API_EXTERNAL_URL")?,"iat":now,"exp":now+300}),
        required(&env, "JWT_SECRET")?,
    )?;
    let response = local_api(io, &env, "/auth/v1/user", Some(&token), None)?;
    if response.status != 200 || response.body["id"] != id {
        return Err("Existing account Auth token check failed".into());
    }
    let response = local_api(
        io,
        &env,
        &format!("/rest/v1/profiles?select=id&id=eq.{id}"),
        Some(&token),
        None,
    )?;
    if response.status != 200
        || response
            .body
            .as_array()
            .is_none_or(|rows| rows.len() != 1 || rows[0]["id"] != id)
    {
        return Err("Own profile RLS check failed".into());
    }
    if local_api(io, &env, "/functions/v1/gpu-status", None, None)?.status != 200 {
        return Err("GPU status function check failed".into());
    }
    if ![401, 403].contains(
        &local_api(
            io,
            &env,
            "/functions/v1/account-delete",
            None,
            Some(json!({})),
        )?
        .status,
    ) {
        return Err("Unauthenticated account deletion was not rejected".into());
    }
    Ok(
        json!({"ok":true,"checks":["auth","anonymous_rls","existing_account","own_profile","gpu_status","delete_rejected"],"recovery_tokens_modified":false,"email_sent":false}),
    )
}
pub fn firebase_assertion(account: &Value, now: i64) -> Result<String> {
    let endpoint = account["token_uri"]
        .as_str()
        .ok_or("Missing Firebase token endpoint")?;
    let url = reqwest::Url::parse(endpoint).map_err(|_| "Invalid Firebase token endpoint")?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("oauth2.googleapis.com" | "accounts.google.com")
        )
        || !matches!(url.path(), "/token" | "/o/oauth2/token")
    {
        return Err("Firebase token endpoint must be an official Google OAuth endpoint".into());
    }
    let pem = account["private_key"]
        .as_str()
        .ok_or("Missing Firebase signing key")?;
    let key = RsaPrivateKey::from_pkcs8_pem(pem)
        .or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem))
        .map_err(|_| "Invalid Firebase private key")?;
    if key.n().bits() < 2048 {
        return Err("Firebase RSA key must be at least2048bits".into());
    }
    let email = account["client_email"]
        .as_str()
        .ok_or("Missing Firebase service account")?;
    let claims = json!({"iss":email,"scope":"https://www.googleapis.com/auth/firebase.messaging","aud":endpoint,"iat":now,"exp":now+300});
    let unsigned = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#),
        URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).map_err(|_| "Cannot encode Firebase claims")?)
    );
    let signature = SigningKey::<Sha256>::new(key).sign(unsigned.as_bytes());
    Ok(format!(
        "{unsigned}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}
pub fn verify_push(root: &Path, io: &mut impl Operator) -> Result<Value> {
    let secrets = read_json(&root.join("private/source-function-secrets.json"))?;
    let account: Value = serde_json::from_str(
        secrets["FIREBASE_SERVICE_ACCOUNT_JSON"]
            .as_str()
            .ok_or("Firebase service account unavailable")?,
    )
    .map_err(|_| "Invalid Firebase service account JSON")?;
    let assertion = firebase_assertion(&account, chrono::Utc::now().timestamp())?;
    let form = reqwest::Url::parse_with_params(
        "https://private.invalid/",
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ],
    )
    .map_err(|_| "Cannot encode Firebase exchange")?
    .query()
    .ok_or("Cannot encode Firebase exchange")?
    .as_bytes()
    .to_vec();
    let response = io.http(
        account["token_uri"]
            .as_str()
            .ok_or("Missing Firebase token endpoint")?,
        "POST",
        &BTreeMap::from([(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        )]),
        Some(form),
    )?;
    if response.status != 200
        || response.body["access_token"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err("Firebase credentials were not authenticated; token omitted".into());
    }
    Ok(json!({"ok":true,"credentials_authenticated":true,"notification_sent":false}))
}
pub fn verify_recovery(root: &Path, allow_email: bool, io: &mut impl Operator) -> Result<Value> {
    if !allow_email {
        return Err(
            "Simulator recovery requires --allow-test-email; no account or email changed".into(),
        );
    }
    let env = env_file(&root.join(".env"))?;
    if sql(
        io,
        "SELECT count(*) FROM auth.users WHERE email='delivered@resend.dev';",
        "supabase_admin",
    )?
    .trim()
        != "0"
    {
        return Err("Simulator account already exists; refusing to modify it".into());
    }
    let identifier = uuid::Uuid::new_v4().to_string();
    let password = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let insert = format!(
        "BEGIN;SET LOCAL session_replication_role=replica;INSERT INTO auth.users(instance_id,id,aud,role,email,encrypted_password,email_confirmed_at,confirmation_token,recovery_token,email_change_token_new,email_change,raw_app_meta_data,raw_user_meta_data,created_at,updated_at)VALUES('00000000-0000-0000-0000-000000000000','{identifier}','authenticated','authenticated','delivered@resend.dev',extensions.crypt('{password}',extensions.gen_salt('bf')),now(),'','','','','{{\"provider\":\"email\",\"providers\":[\"email\"]}}','{{}}',now(),now());INSERT INTO auth.identities(provider_id,user_id,identity_data,provider,created_at,updated_at)VALUES('{identifier}','{identifier}','{{\"sub\":\"{identifier}\",\"email\":\"delivered@resend.dev\",\"email_verified\":true}}','email',now(),now());COMMIT;"
    );
    sql(io, &insert, "supabase_admin")?;
    let result = (|| {
        let mut ready = false;
        for _ in 0..30 {
            if local_api(io, &env, "/auth/v1/health", None, None).is_ok_and(|r| r.status == 200) {
                ready = true;
                break;
            }
            io.wait(Duration::from_secs(1));
        }
        if !ready {
            return Err("Auth recovery simulator health check failed".into());
        }
        let response = local_api(
            io,
            &env,
            "/auth/v1/recover?redirect_to=https%3A%2F%2Fcbgpu.com%2Fauth%2Freset-password",
            None,
            Some(json!({"email":"delivered@resend.dev"})),
        )?;
        if response.status != 200 {
            return Err(
                "Native simulator recovery endpoint failed; private response omitted".into(),
            );
        }
        if sql(io,&format!("SELECT recovery_sent_at IS NOT NULL AND length(recovery_token)>0 FROM auth.users WHERE id='{identifier}';"),"supabase_admin")?.trim()!="t"{return Err("Recovery state was not committed".into())}
        Ok(json!({"ok":true,"simulator_recovery_verified":true,"real_user_email_sent":false}))
    })();
    let cleanup = sql(
        io,
        &format!(
            "DELETE FROM auth.users WHERE id='{identifier}' AND email='delivered@resend.dev';"
        ),
        "supabase_admin",
    );
    if cleanup.is_err() {
        return Err(
            "Simulator cleanup failed; inspect only the test account administratively".into(),
        );
    }
    result
}
pub fn restart_controller(marker: &Path, allow: bool, io: &mut impl Operator) -> Result<Value> {
    if !allow {
        return Err("Controller restart requires --allow-controller-restart".into());
    }
    if !io.is_root() {
        return Err("Controller restart requires root".into());
    }
    if sql(io,"select count(*) from public.reservations where status='approved' and start_time<=clock_timestamp() and end_time>clock_timestamp();","postgres")?.trim()!="0"{return Err("Active GPU reservations; controller restart is prohibited".into())}
    let inspect = strings(&[
        "docker",
        "inspect",
        "--format",
        "{{.Name}} {{.State.Running}} {{.State.StartedAt}}",
        "jupiterhub_gpu1",
        "jupiterhub_gpu2",
        "jupiterhub_gpu3",
        "jupiterhub_gpu4",
    ]);
    let before = io.run(&inspect, None, None, None, 20)?;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(marker)
        .map_err(|_| "Preservation marker exists or cannot be created")?;
    drop(file);
    let result = (|| {
        io.run(
            &strings(&["systemctl", "restart", "cbgpu-daemon.service"]),
            None,
            None,
            None,
            90,
        )?;
        io.wait(Duration::from_secs(35));
        io.run(
            &strings(&["systemctl", "is-active", "--quiet", "cbgpu-daemon.service"]),
            None,
            None,
            None,
            15,
        )?;
        let after = io.run(&inspect, None, None, None, 20)?;
        if before != after {
            return Err(
                "GPU container state changed unexpectedly; inspect operations before proceeding"
                    .into(),
            );
        }
        Ok(json!({"ok":true,"controller_restarted":true,"gpu_container_state_preserved":true}))
    })();
    fs::remove_file(marker).map_err(|_| "Cannot remove owned preservation marker")?;
    result
}
pub fn valid_root(relative: &str) -> bool {
    relative.len() <= 500
        && !Path::new(relative).is_absolute()
        && !relative.contains(['\\', '\0', '\n', '\r'])
        && !relative.split('/').any(|p| p == "..")
}
pub fn verify_server_runtimes(io: &mut impl Operator) -> Result<Value> {
    let fixture = tempfile::tempdir().map_err(|_| "Cannot create isolated runtime fixture")?;
    let root = fixture.path();
    fs::create_dir(root.join("backend api")).map_err(|_| "Cannot create nested runtime fixture")?;
    fs::write(root.join("backend api/sentinel"), "correct-directory")
        .map_err(|_| "Cannot write runtime fixture")?;
    std::os::unix::fs::symlink("/etc", root.join("escape"))
        .map_err(|_| "Cannot create isolated symlink fixture")?;
    cbgpu_front_worker::within(root, "backend api")?;
    if cbgpu_front_worker::within(root, "escape").is_ok() {
        return Err("Native worker accepted symlink escape".into());
    }
    for bad in ["/etc", "../outside", "x/../outside", "folder\\other"] {
        if valid_root(bad) {
            return Err("Unsafe root accepted".into());
        }
    }
    let mount = format!("type=bind,src={},dst=/app,readonly", root.display());
    let setup = "set -e; base=$(realpath /app); root=$(realpath '/app/backend api'); case \"$root\" in \"$base\"|\"$base\"/*) ;; *) exit 42;; esac; cd \"$root\"; cat sentinel";
    let output = io.run(
        &strings(&[
            "docker",
            "run",
            "--rm",
            "--network",
            "none",
            "--mount",
            &mount,
            "python:3.12-slim",
            "sh",
            "-c",
            setup,
        ]),
        None,
        None,
        None,
        120,
    )?;
    if output.trim() != "correct-directory" {
        return Err("Nested directory runtime test failed".into());
    }
    let blocked = "set -e; base=$(realpath /app); root=$(realpath /app/escape); case \"$root\" in \"$base\"|\"$base\"/*) exit 0;; *) exit 42;; esac";
    if io
        .run(
            &strings(&[
                "docker",
                "run",
                "--rm",
                "--network",
                "none",
                "--mount",
                &mount,
                "python:3.12-slim",
                "sh",
                "-c",
                blocked,
            ]),
            None,
            None,
            None,
            120,
        )
        .is_ok()
    {
        return Err("Container symlink escape was not rejected".into());
    }
    let repository = root.join("spring");
    io.run(
        &strings(&[
            "git",
            "clone",
            "--depth=1",
            "https://github.com/spring-guides/gs-rest-service",
            repository.to_str().ok_or("Invalid fixture path")?,
        ]),
        None,
        None,
        None,
        180,
    )?;
    let identifier = uuid::Uuid::new_v4().to_string();
    let container = format!("cbgpu-admin-test-{identifier}");
    let mount = format!(
        "type=bind,src={},dst=/source,readonly",
        repository.display()
    );
    let result = (|| {
        io.run(&strings(&["docker","run","-d","--name",&container,"--label","cbgpu.test.admin-ops=true","--cpus","2","--memory","2g","--pids-limit","512","--cap-drop","ALL","--security-opt","no-new-privileges","--read-only","--tmpfs","/tmp:rw,size=1g","--tmpfs","/root/.m2:rw,size=1g","--mount",&mount,"-p","127.0.0.1::8080","maven:3.9-eclipse-temurin-21","sh","-c","mkdir -p /tmp/work; cp -a /source/. /tmp/work/; cd /tmp/work/complete; mvn -B -DskipTests package && java -jar target/*.jar"]),None,None,None,120)?;
        let address = io.run(
            &strings(&["docker", "port", &container, "8080/tcp"]),
            None,
            None,
            None,
            20,
        )?;
        let port = address
            .trim()
            .strip_prefix("127.0.0.1:")
            .ok_or("Test container did not bind loopback")?
            .parse::<u16>()
            .map_err(|_| "Invalid test port")?;
        for _ in 0..150 {
            if io
                .http(
                    &format!("http://127.0.0.1:{port}/greeting"),
                    "GET",
                    &BTreeMap::new(),
                    None,
                )
                .is_ok_and(|r| {
                    r.status == 200
                        && r.body["content"]
                            .as_str()
                            .is_some_and(|s| s.starts_with("Hello"))
                })
            {
                return Ok(
                    json!({"ok":true,"nested_roots":true,"symlink_escape_blocked":true,"java21_spring_verified":true}),
                );
            }
            io.wait(Duration::from_secs(2));
        }
        Err("Spring Boot isolated test did not become ready".into())
    })();
    let cleanup = io.run(
        &strings(&["docker", "rm", "-f", &container]),
        None,
        None,
        None,
        30,
    );
    if cleanup.is_err() {
        return Err(
            "Disposable runtime test cleanup failed; production resources were not selected".into(),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::{
        pkcs1v15::{Signature, VerifyingKey},
        pkcs8::EncodePrivateKey,
        pkcs8::LineEnding,
        signature::Verifier,
    };
    use tempfile::tempdir;
    const ID: &str = "00000000-0000-4000-8000-000000000001";
    #[derive(Default)]
    struct Fake {
        commands: Vec<Vec<String>>,
        queries: Vec<String>,
        requests: Vec<(String, String)>,
        root: bool,
        active: bool,
        fail_restart: bool,
        sim_exists: bool,
        fail_recovery: bool,
        rls_leak: bool,
        built: bool,
    }
    impl Operator for Fake {
        fn run(
            &mut self,
            args: &[String],
            _: Option<&Path>,
            _: Option<&BTreeMap<String, String>>,
            input: Option<&[u8]>,
            _: u64,
        ) -> Result<String> {
            self.commands.push(args.to_vec());
            if let Some(input) = input {
                let sql = String::from_utf8_lossy(input).into_owned();
                self.queries.push(sql.clone());
                if sql.contains("count(*) FROM auth.users") {
                    return Ok(if self.sim_exists { "1" } else { "0" }.into());
                }
                if sql.contains("count(*) from public.reservations") {
                    return Ok(if self.active { "1" } else { "0" }.into());
                }
                if sql.contains("recovery_sent_at") {
                    return Ok("t".into());
                }
                if sql.contains("json_build_object") {
                    return Ok(json!({"id":ID,"email":"fixture@example.test"}).to_string());
                }
                return Ok(String::new());
            }
            if args[0] == "dig" {
                return Ok(match args[3].as_str() {
                    "resend._domainkey.cbgpu.com" => format!("\"{}\"", DKIM),
                    "rsend.cbgpu.com" => "rsend-apne1.forge.rmta.net.\n".into(),
                    "send.cbgpu.com" => "send.forge.rmta.net.\n".into(),
                    _ => "\"v=DMARC1; p=none;\"\n".into(),
                });
            }
            if args[0] == "systemctl" && self.fail_restart {
                return Err("synthetic controller failure".into());
            }
            if args.get(1).is_some_and(|s| s == "inspect") {
                return Ok("unchanged GPU containers".into());
            }
            if args.get(1).is_some_and(|s| s == "port") {
                return Ok("127.0.0.1:45678".into());
            }
            if args
                .last()
                .is_some_and(|s| s.contains("realpath /app/escape"))
            {
                return Err("escape blocked".into());
            }
            if args.last().is_some_and(|s| s.contains("cat sentinel")) {
                return Ok("correct-directory".into());
            }
            Ok(String::new())
        }
        fn http(
            &mut self,
            url: &str,
            method: &str,
            _: &BTreeMap<String, String>,
            _: Option<Vec<u8>>,
        ) -> Result<Response> {
            self.requests.push((url.into(), method.into()));
            let (status, body) = if url.contains("/recover?") {
                (if self.fail_recovery { 500 } else { 200 }, json!({}))
            } else if url.contains("/auth/v1/user") {
                (200, json!({"id":ID}))
            } else if url.contains("/profiles?select=id&id=") {
                (200, json!([{"id":ID}]))
            } else if url.contains("/profiles?select=id") || url.contains("/gpu_ssh_keys") {
                (
                    200,
                    if self.rls_leak {
                        json!([{"id":"must-not-leak"}])
                    } else {
                        json!([])
                    },
                )
            } else if url.contains("account-delete") {
                (401, json!({}))
            } else if url.contains("googleapis.com/token") {
                (200, json!({"access_token":"synthetic-only"}))
            } else if url.contains("/greeting") {
                (200, json!({"content":"Hello, fixture!"}))
            } else {
                (200, json!({}))
            };
            Ok(Response { status, body })
        }
        fn build(&mut self, _: &Path, env: &BTreeMap<String, String>, _: &Path) -> Result<()> {
            assert_eq!(env["NEXT_BUILD_DIR"], ".next-email-ops");
            self.built = true;
            Ok(())
        }
        fn wait(&mut self, _: Duration) {}
        fn is_root(&self) -> bool {
            self.root
        }
    }
    fn fixture(root: &Path) {
        fs::create_dir_all(root.join("private")).unwrap();
        fs::write(root.join(".env"),"SUPABASE_PUBLISHABLE_KEY=synthetic-public\nANON_KEY=synthetic-anon\nSERVICE_ROLE_KEY=synthetic-service-role\nJWT_SECRET=synthetic-jwt-secret\nAPI_EXTERNAL_URL=https://cbgpu.com/backend\n").unwrap();
        fs::write(
            root.join("private/source-function-secrets.json"),
            json!({"EDUSET_TOKEN_SECRET":"synthetic-eduset","RESEND_API_KEY":"synthetic-resend"})
                .to_string(),
        )
        .unwrap();
        fs::write(
            root.join("private/developer-portal.json"),
            json!({"ingestSecret":"synthetic-ingest"}).to_string(),
        )
        .unwrap();
        fs::write(
            root.join("private/email-sender-status.json"),
            json!({"active":"info@cbgpu.com"}).to_string(),
        )
        .unwrap();
    }
    #[test]
    fn every_command_defaults_to_plan_and_historical_data_replays_are_retired() {
        for command in [
            "activate-local-backend",
            "final-sync",
            "configure-scheduled-jobs",
        ] {
            let p = plan(command).unwrap();
            assert_eq!(p["retired"], true);
            assert_eq!(p["mode"], "plan");
        }
        for command in [
            "check-email-domain",
            "build-frontend",
            "verify-backend",
            "verify-push-credentials",
            "verify-recovery-simulator",
            "restart-controller-preserving-gpus",
            "verify-server-runtimes",
        ] {
            let p = plan(command).unwrap();
            assert_eq!(p["requires_apply"], true);
            assert_eq!(p["mode"], "plan");
        }
    }
    #[test]
    fn dns_txt_chunks_and_domain_status_match_exact_values() {
        assert_eq!(parse_txt("\"long\" \"key\"").unwrap(), "longkey");
        assert_eq!(
            parse_txt("\"v=DMARC1; p=none;\"").unwrap(),
            "v=DMARC1; p=none;"
        );
        assert!(parse_txt("\"unfinished").is_err());
        let tmp = tempdir().unwrap();
        let mut io = Fake::default();
        let result = check_email_domain(tmp.path(), &mut io).unwrap();
        assert_eq!(result["dns_ready"], true);
        assert!(io.requests.is_empty());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(tmp.path().join("private/email-domain-status.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn frontend_build_keeps_secrets_private_and_never_restarts_or_deploys() {
        let tmp = tempdir().unwrap();
        fixture(tmp.path());
        let frontend = tmp.path().join("web");
        fs::create_dir(&frontend).unwrap();
        fs::write(frontend.join("package.json"), "{}").unwrap();
        let mut io = Fake::default();
        let result = build_frontend(tmp.path(), &frontend, &mut io).unwrap();
        assert!(io.built);
        assert_eq!(result["deployed"], false);
        assert!(io.commands.is_empty());
        let env = fs::read_to_string(tmp.path().join("private/frontend-runtime.env")).unwrap();
        assert!(env.contains("CBGPU <info@cbgpu.com>"));
        assert!(!result.to_string().contains("synthetic-service-role"));
    }
    #[test]
    fn backend_verification_preserves_existing_recovery_tokens_and_checks_rls() {
        let tmp = tempdir().unwrap();
        fixture(tmp.path());
        let mut io = Fake::default();
        assert_eq!(
            verify_backend(tmp.path(), &mut io).unwrap()["recovery_tokens_modified"],
            false
        );
        assert!(
            !io.requests
                .iter()
                .any(|(u, _)| u.contains("generate_link") || u.contains("recover?"))
        );
        assert!(io.queries.iter().all(|q| q.starts_with("select ")));
        io.rls_leak = true;
        let error = verify_backend(tmp.path(), &mut io).unwrap_err();
        assert!(!error.contains("must-not-leak"));
    }
    #[test]
    fn hmac_jwt_is_valid_and_short_lived() {
        let claims = json!({"sub":ID,"exp":1000});
        let token = jwt_hs256(&claims, "synthetic-key").unwrap();
        let parts: Vec<_> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let mut mac = Hmac::<Sha256>::new_from_slice(b"synthetic-key").unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        mac.verify_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap())
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap(),
            claims
        );
    }
    #[test]
    fn simulator_requires_explicit_email_optin_refuses_existing_and_cleans_up_failures() {
        let tmp = tempdir().unwrap();
        fixture(tmp.path());
        let mut io = Fake::default();
        assert!(verify_recovery(tmp.path(), false, &mut io).is_err());
        assert!(io.commands.is_empty());
        io.sim_exists = true;
        assert!(verify_recovery(tmp.path(), true, &mut io).is_err());
        assert!(!io.queries.iter().any(|q| q.contains("INSERT INTO")));
        io.sim_exists = false;
        io.fail_recovery = true;
        assert!(verify_recovery(tmp.path(), true, &mut io).is_err());
        assert!(
            io.queries
                .iter()
                .any(|q| q.starts_with("DELETE FROM auth.users WHERE id='")
                    && q.contains("AND email='delivered@resend.dev'"))
        );
        assert!(!io.queries.iter().any(|q| q.contains("TRUNCATE")));
    }
    #[test]
    fn controller_restart_refuses_live_reservations_and_cleans_owned_marker_on_failure() {
        let tmp = tempdir().unwrap();
        let marker = tmp.path().join("marker");
        let mut io = Fake {
            root: true,
            active: true,
            ..Default::default()
        };
        assert!(restart_controller(&marker, true, &mut io).is_err());
        assert!(!io.commands.iter().any(|a| a[0] == "systemctl"));
        io.active = false;
        io.fail_restart = true;
        assert!(restart_controller(&marker, true, &mut io).is_err());
        assert!(!marker.exists());
        assert!(
            !io.commands
                .iter()
                .any(|a| a.get(1).is_some_and(|s| s == "restart") && a[0] == "docker")
        );
    }
    #[test]
    fn runtime_verifier_selects_only_disposable_labeled_containers() {
        let mut io = Fake::default();
        assert_eq!(
            verify_server_runtimes(&mut io).unwrap()["java21_spring_verified"],
            true
        );
        let cleanup = io
            .commands
            .iter()
            .find(|a| a.get(1).is_some_and(|s| s == "rm"))
            .unwrap();
        assert!(cleanup.last().unwrap().starts_with("cbgpu-admin-test-"));
        assert!(
            !io.commands
                .iter()
                .any(|a| a.iter().any(|v| v.starts_with("jupiterhub")))
        );
        assert!(
            io.commands
                .iter()
                .any(|a| a.iter().any(|v| v == "cbgpu.test.admin-ops=true"))
        );
    }
    #[test]
    fn firebase_rs256_exchange_never_sends_notifications() {
        let tmp = tempdir().unwrap();
        fixture(tmp.path());
        let private = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
        let account = json!({"client_email":"fixture@example.test","token_uri":"https://oauth2.googleapis.com/token","private_key":private.to_pkcs8_pem(LineEnding::LF).unwrap().to_string()});
        let assertion = firebase_assertion(&account, 100).unwrap();
        let parts: Vec<_> = assertion.split('.').collect();
        let signature =
            Signature::try_from(URL_SAFE_NO_PAD.decode(parts[2]).unwrap().as_slice()).unwrap();
        VerifyingKey::<Sha256>::new(private.to_public_key())
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .unwrap();
        fs::write(
            tmp.path().join("private/source-function-secrets.json"),
            json!({"FIREBASE_SERVICE_ACCOUNT_JSON":account.to_string()}).to_string(),
        )
        .unwrap();
        let mut io = Fake::default();
        assert_eq!(
            verify_push(tmp.path(), &mut io).unwrap()["notification_sent"],
            false
        );
        assert_eq!(io.requests.len(), 1);
        assert!(io.requests[0].0.ends_with("googleapis.com/token"));
        let mut foreign = account;
        foreign["token_uri"] = json!("https://attacker.invalid/token");
        assert!(firebase_assertion(&foreign, 100).is_err());
    }
}
