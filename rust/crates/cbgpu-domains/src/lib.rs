//! Native replacements for the legacy project-domain and external-TLS adapters.
//! Credentials and provider error bodies are never included in user-visible errors.
use rand::{RngCore, rngs::OsRng};
use regex::Regex;
use reqwest::{Method, blocking::Client, redirect::Policy};
use serde_json::{Value, json};
use std::{
    fmt, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub const ACCOUNT: &str = "3a4fa468d49633b9a24c2c40539ea8a5";
pub const ZONE: &str = "99d6ebc0a8271e9b2104245b25c47529";
pub const WORKER: &str = "cbgpu-project-domains";
pub const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
pub const INCLUDED_HOSTNAME_LIMIT: usize = 90;
const BASE: &str = "https://api.cloudflare.com/client/v4/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainError(pub String);
impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for DomainError {}
fn failure(message: &str) -> DomainError {
    DomainError(message.into())
}

#[derive(Clone)]
pub struct Settings {
    pub root: PathBuf,
    pub token_file: PathBuf,
    pub account: String,
    pub zone: String,
    pub worker: String,
}
impl Settings {
    pub fn for_root(root: PathBuf) -> Self {
        Self {
            token_file: root.join("private/deploy-cloudflare-token.json"),
            root,
            account: ACCOUNT.into(),
            zone: ZONE.into(),
            worker: WORKER.into(),
        }
    }
    fn validate(&self) -> Result<(), DomainError> {
        for id in [&self.account, &self.zone] {
            if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(failure("Invalid domain provider configuration"));
            }
        }
        if self.worker.is_empty()
            || self.worker.len() > 63
            || !self
                .worker
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(failure("Invalid domain provider configuration"));
        }
        Ok(())
    }
}

pub enum Body {
    Json(Value),
    Bytes(Vec<u8>),
}
pub trait DomainApi {
    fn request(
        &self,
        route: &str,
        method: Method,
        body: Option<Body>,
        content_type: &str,
    ) -> Result<Value, DomainError>;
}

pub struct CloudflareApi {
    client: Client,
    token: String,
}
impl CloudflareApi {
    pub fn from_file(path: &Path) -> Result<Self, DomainError> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| failure("Domain provider credentials are unavailable"))?;
        if !metadata.is_file() || metadata.len() > 65_536 {
            return Err(failure("Invalid domain provider credentials"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(failure(
                    "Domain provider credential file must be private (mode 600)",
                ));
            }
        }
        let contents =
            fs::read(path).map_err(|_| failure("Domain provider credentials are unavailable"))?;
        let value: Value = serde_json::from_slice(&contents)
            .map_err(|_| failure("Invalid domain provider credentials"))?;
        let token = value
            .get("token")
            .and_then(Value::as_str)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 4096
                    && value.bytes().all(|b| b.is_ascii_graphic())
            })
            .ok_or_else(|| failure("Invalid domain provider credentials"))?
            .to_owned();
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .redirect(Policy::none())
            .build()
            .map_err(|_| failure("Domain service client unavailable"))?;
        Ok(Self { client, token })
    }
}
impl DomainApi for CloudflareApi {
    fn request(
        &self,
        route: &str,
        method: Method,
        body: Option<Body>,
        content_type: &str,
    ) -> Result<Value, DomainError> {
        // Routes are constructed by this module, never passed through from a caller.
        if route.starts_with('/')
            || route.contains("..")
            || route.contains('#')
            || route.contains("://")
        {
            return Err(failure("Invalid domain service route"));
        }
        let mut request = self
            .client
            .request(method, format!("{BASE}{route}"))
            .bearer_auth(&self.token)
            .header("Content-Type", content_type);
        if let Some(body) = body {
            request = match body {
                Body::Json(value) => request.json(&value),
                Body::Bytes(bytes) => request.body(bytes),
            };
        }
        let response = request
            .send()
            .map_err(|_| failure("Domain service connection failed"))?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| failure("Domain service response could not be read"))?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(failure("Domain service response exceeds the allowed size"));
        }
        parse_response(status.as_u16(), &bytes)
    }
}

pub fn parse_response(status: u16, bytes: &[u8]) -> Result<Value, DomainError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| DomainError(format!("Domain service returned HTTP {status}")))?;
    if !(200..300).contains(&status) || value.get("success") != Some(&Value::Bool(true)) {
        // Error messages can echo Authorization headers or credentials. Keep only numeric provider codes.
        let codes: Vec<String> = value
            .get("errors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|error| error.get("code").and_then(Value::as_u64))
            .take(8)
            .map(|code| code.to_string())
            .collect();
        return Err(DomainError(if codes.is_empty() {
            format!("Domain service rejected the request (HTTP {status})")
        } else {
            format!(
                "Domain service rejected the request (HTTP {status}; codes {})",
                codes.join(",")
            )
        }));
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}
fn call(
    api: &impl DomainApi,
    route: &str,
    method: Method,
    body: Option<Value>,
) -> Result<Value, DomainError> {
    api.request(route, method, body.map(Body::Json), "application/json")
}
fn array(value: &Value) -> Result<&Vec<Value>, DomainError> {
    value
        .as_array()
        .ok_or_else(|| failure("Domain service returned an invalid list"))
}
fn identifier(value: &str) -> Result<(), DomainError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(failure("Invalid provider hostname identifier"));
    }
    Ok(())
}
pub fn validate_project_host(host: &str) -> Result<(), DomainError> {
    let pattern = Regex::new(r"^[a-z][a-z0-9-]{2,60}\.(front|server)\.cbgpu\.com$")
        .expect("constant domain regex");
    if !pattern.is_match(host) {
        return Err(failure("Invalid project domain"));
    }
    Ok(())
}
pub fn validate_external_host(host: &str) -> Result<(), DomainError> {
    if host.len() > 253
        || !host.contains('.')
        || host.bytes().any(|b| !b.is_ascii())
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(failure("Invalid custom hostname"));
    }
    Ok(())
}

pub fn ensure_project(
    api: &impl DomainApi,
    settings: &Settings,
    host: &str,
) -> Result<Value, DomainError> {
    settings.validate()?;
    validate_project_host(host)?;
    let route = format!("accounts/{}/workers/domains", settings.account);
    let existing = call(api, &route, Method::GET, None)?;
    if let Some(found) = array(&existing)?
        .iter()
        .find(|value| value.get("hostname").and_then(Value::as_str) == Some(host))
    {
        if found.get("service").and_then(Value::as_str) != Some(&settings.worker) {
            return Err(failure("Existing domain belongs to another application"));
        }
        return Ok(found.clone());
    }
    call(
        api,
        &route,
        Method::PUT,
        Some(
            json!({"hostname":host,"service":settings.worker,"environment":"production","zone_id":settings.zone}),
        ),
    )
}
pub fn remove_project(
    api: &impl DomainApi,
    settings: &Settings,
    host: &str,
) -> Result<(), DomainError> {
    settings.validate()?;
    validate_project_host(host)?;
    let route = format!("accounts/{}/workers/domains", settings.account);
    let existing = call(api, &route, Method::GET, None)?;
    for value in array(&existing)? {
        if value.get("hostname").and_then(Value::as_str) == Some(host)
            && value.get("service").and_then(Value::as_str) == Some(&settings.worker)
        {
            let id = value
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| failure("Domain service returned an invalid hostname"))?;
            identifier(id)?;
            call(api, &format!("{route}/{id}"), Method::DELETE, None)?;
        }
    }
    Ok(())
}

fn random_hex(bytes: usize) -> Result<String, DomainError> {
    let mut values = vec![0; bytes];
    OsRng
        .try_fill_bytes(&mut values)
        .map_err(|_| failure("Secure randomness unavailable"))?;
    Ok(values.iter().map(|value| format!("{value:02x}")).collect())
}
pub fn ensure_edge_key(path: &Path) -> Result<(), DomainError> {
    // Atomic O_EXCL matches the legacy behavior and never overwrites an existing key.
    if path
        .try_exists()
        .map_err(|_| failure("Deployment edge key unavailable"))?
    {
        return Ok(());
    }
    let key = random_hex(32)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => file
            .write_all(key.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| failure("Could not save deployment edge key")),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => Err(failure("Could not create deployment edge key")),
    }
}
pub fn worker_multipart(code: &[u8], boundary: &str) -> Vec<u8> {
    let metadata = serde_json::to_vec(
        &json!({"main_module":"worker.js","compatibility_date":"2026-10-29","bindings":[]}),
    )
    .expect("static metadata");
    let mut body = Vec::new();
    for (name, mime, data) in [
        ("metadata", "application/json", metadata.as_slice()),
        ("worker.js", "application/javascript+module", code),
    ] {
        write!(&mut body, "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n").expect("writing memory");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    write!(&mut body, "--{boundary}--\r\n").expect("writing memory");
    body
}
pub fn setup_project_worker(api: &impl DomainApi, settings: &Settings) -> Result<(), DomainError> {
    settings.validate()?;
    let code = fs::read(settings.root.join("deploy/edge-worker.js")).map_err(|_| {
        failure("Compiled deploy/edge-worker.js is unavailable; build browser assets first")
    })?;
    if code.len() > 5 * 1024 * 1024 {
        return Err(failure("Worker source exceeds the allowed size"));
    }
    ensure_edge_key(&settings.root.join("private/deployment-edge-key"))?;
    let boundary = format!("cbgpu-{}", random_hex(16)?);
    api.request(
        &format!(
            "accounts/{}/workers/scripts/{}",
            settings.account, settings.worker
        ),
        Method::PUT,
        Some(Body::Bytes(worker_multipart(&code, &boundary))),
        &format!("multipart/form-data; boundary={boundary}"),
    )?;
    Ok(())
}

pub fn tls_output(value: &Value) -> Result<Value, DomainError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| failure("Domain service returned an invalid hostname"))?;
    identifier(id)?;
    let mut records = Vec::new();
    if let Some(ownership) = value
        .get("ownership_verification")
        .filter(|value| value.is_object())
        && let (Some(name), Some(value)) = (
            ownership
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty()),
            ownership
                .get("value")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty()),
        )
    {
        let record_type = ownership
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("TXT")
            .to_uppercase();
        records.push(json!({"type":record_type,"name":name,"value":value}));
    }
    if let Some(validation) = value
        .pointer("/ssl/validation_records")
        .and_then(Value::as_array)
    {
        for record in validation {
            if let (Some(name), Some(value)) = (
                record
                    .get("txt_name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty()),
                record
                    .get("txt_value")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty()),
            ) {
                records.push(json!({"type":"TXT","name":name,"value":value}));
            }
        }
    }
    let active = value.get("status").and_then(Value::as_str) == Some("active")
        && value.pointer("/ssl/status").and_then(Value::as_str) == Some("active");
    Ok(json!({"id":id,"status":if active {"active"} else {"pending_tls"},"records":records}))
}
pub fn ensure_custom_tls(
    api: &impl DomainApi,
    settings: &Settings,
    host: &str,
    provider_id: Option<&str>,
) -> Result<Value, DomainError> {
    settings.validate()?;
    validate_external_host(host)?;
    let route = format!("zones/{}/custom_hostnames", settings.zone);
    let value = if let Some(id) = provider_id.filter(|id| !id.is_empty()) {
        identifier(id)?;
        call(api, &format!("{route}/{id}"), Method::GET, None)?
    } else {
        let current = call(api, &format!("{route}?per_page=100"), Method::GET, None)?;
        let current = array(&current)?;
        if let Some(found) = current
            .iter()
            .find(|value| value.get("hostname").and_then(Value::as_str) == Some(host))
        {
            found.clone()
        } else {
            // Never enable paid capacity, billing or automatic upgrades.
            if current.len() >= INCLUDED_HOSTNAME_LIMIT {
                return Err(failure("Included hostname capacity reached"));
            }
            let fallback = call(api, &format!("{route}/fallback_origin"), Method::GET, None)?;
            if fallback.get("status").and_then(Value::as_str) != Some("active") {
                return Err(failure(
                    "Custom TLS fallback origin needs administrator configuration",
                ));
            }
            call(
                api,
                &route,
                Method::POST,
                Some(
                    json!({"hostname":host,"ssl":{"method":"http","type":"dv","settings":{"min_tls_version":"1.2"}}}),
                ),
            )?
        }
    };
    // An existing provider ID must not accidentally grant the status of a different hostname.
    if let Some(actual) = value.get("hostname").and_then(Value::as_str)
        && actual != host
    {
        return Err(failure(
            "Custom hostname does not match its provider identifier",
        ));
    }
    tls_output(&value)
}
pub fn remove_custom_tls(
    api: &impl DomainApi,
    settings: &Settings,
    id: &str,
) -> Result<Value, DomainError> {
    settings.validate()?;
    identifier(id)?;
    call(
        api,
        &format!("zones/{}/custom_hostnames/{id}", settings.zone),
        Method::DELETE,
        None,
    )?;
    Ok(json!({}))
}
/// Serialize local provider capacity checks and writes across concurrently spawned adapters.
/// Dropping the file (including process termination) releases the advisory lock.
pub fn provider_lock(root: &Path) -> Result<fs::File, DomainError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(root.join("private/custom-hostnames.lock"))
            .map_err(|_| failure("Domain provider capacity lock unavailable"))?;
        if !file
            .metadata()
            .map_err(|_| failure("Domain provider capacity lock unavailable"))?
            .is_file()
        {
            return Err(failure("Invalid domain provider capacity lock"));
        }
        rustix::fs::flock(&file, rustix::fs::FlockOperation::LockExclusive)
            .map_err(|_| failure("Domain provider capacity lock unavailable"))?;
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Err(failure(
            "Domain provider capacity locking is unavailable on this platform",
        ))
    }
}
pub fn setup_required() -> Value {
    json!({"status":"provider_setup_required","records":[]})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque};
    struct Expected {
        method: Method,
        route: String,
        result: Result<Value, DomainError>,
    }
    struct Mock {
        expected: RefCell<VecDeque<Expected>>,
        bodies: RefCell<Vec<Option<Value>>>,
    }
    impl Mock {
        fn new(values: Vec<(Method, String, Value)>) -> Self {
            Self {
                expected: RefCell::new(
                    values
                        .into_iter()
                        .map(|(method, route, value)| Expected {
                            method,
                            route,
                            result: Ok(value),
                        })
                        .collect(),
                ),
                bodies: RefCell::new(Vec::new()),
            }
        }
        fn done(&self) {
            assert!(self.expected.borrow().is_empty());
        }
    }
    impl DomainApi for Mock {
        fn request(
            &self,
            route: &str,
            method: Method,
            body: Option<Body>,
            _: &str,
        ) -> Result<Value, DomainError> {
            let expected = self
                .expected
                .borrow_mut()
                .pop_front()
                .expect("unexpected provider request");
            assert_eq!(expected.method, method);
            assert_eq!(expected.route, route);
            self.bodies.borrow_mut().push(match body {
                Some(Body::Json(value)) => Some(value),
                _ => None,
            });
            expected.result
        }
    }
    fn settings() -> Settings {
        Settings::for_root(PathBuf::from("/unused-synthetic-backend"))
    }
    fn workers() -> String {
        format!("accounts/{ACCOUNT}/workers/domains")
    }
    fn custom() -> String {
        format!("zones/{ZONE}/custom_hostnames")
    }
    fn active() -> Value {
        json!({"id":"provider-123","hostname":"research.example.org","status":"active","ssl":{"status":"active"}})
    }

    #[test]
    fn exact_project_hostname_validation_matches_legacy_namespace() {
        for host in [
            "abc.front.cbgpu.com",
            "project-123.server.cbgpu.com",
            "abc-12345678.front.cbgpu.com",
        ] {
            assert!(validate_project_host(host).is_ok());
        }
        for host in [
            "ab.front.cbgpu.com",
            "ABC.front.cbgpu.com",
            "www.cbgpu.com",
            "x.front.cbgpu.com.evil.org",
            "abc.front.cbgpu.com/anything",
            "../abc.front.cbgpu.com",
        ] {
            assert!(validate_project_host(host).is_err());
        }
        assert!(validate_project_host(&format!("{}.front.cbgpu.com", "a".repeat(62))).is_err());
    }
    #[test]
    fn existing_worker_domain_is_reused_without_provider_writes() {
        let found = json!({"hostname":"demo.front.cbgpu.com","service":WORKER,"id":"123"});
        let api = Mock::new(vec![(Method::GET, workers(), json!([found]))]);
        assert_eq!(
            ensure_project(&api, &settings(), "demo.front.cbgpu.com").unwrap()["id"],
            "123"
        );
        api.done();
    }
    #[test]
    fn unrelated_workers_domain_is_not_taken_over() {
        let api = Mock::new(vec![(
            Method::GET,
            workers(),
            json!([{"hostname":"demo.front.cbgpu.com","service":"other-application"}]),
        )]);
        assert_eq!(
            ensure_project(&api, &settings(), "demo.front.cbgpu.com")
                .unwrap_err()
                .0,
            "Existing domain belongs to another application"
        );
        api.done();
    }
    #[test]
    fn missing_project_domain_is_registered_with_exact_binding() {
        let api = Mock::new(vec![
            (Method::GET, workers(), json!([])),
            (Method::PUT, workers(), json!({"id":"new-domain"})),
        ]);
        ensure_project(&api, &settings(), "demo.server.cbgpu.com").unwrap();
        assert_eq!(
            api.bodies.borrow()[1],
            Some(
                json!({"hostname":"demo.server.cbgpu.com","service":WORKER,"environment":"production","zone_id":ZONE})
            )
        );
        api.done();
    }
    #[test]
    fn removal_only_deletes_our_matching_worker_domain() {
        let api = Mock::new(vec![
            (
                Method::GET,
                workers(),
                json!([{"hostname":"other.front.cbgpu.com","service":WORKER,"id":"keep"},{"hostname":"demo.front.cbgpu.com","service":"other-worker","id":"keep-other"},{"hostname":"demo.front.cbgpu.com","service":WORKER,"id":"delete-id"}]),
            ),
            (
                Method::DELETE,
                format!("{}/delete-id", workers()),
                json!({}),
            ),
        ]);
        remove_project(&api, &settings(), "demo.front.cbgpu.com").unwrap();
        api.done();
    }
    #[test]
    fn custom_tls_reuses_existing_hostname_at_capacity() {
        let mut list: Vec<Value> = (0..90)
            .map(|index| json!({"hostname":format!("host{index}.example.org")}))
            .collect();
        list.push(active());
        let api = Mock::new(vec![(
            Method::GET,
            format!("{}?per_page=100", custom()),
            json!(list),
        )]);
        assert_eq!(
            ensure_custom_tls(&api, &settings(), "research.example.org", None).unwrap()["status"],
            "active"
        );
        api.done();
    }
    #[test]
    fn custom_tls_never_enables_paid_capacity_or_creates_after_quota() {
        let api = Mock::new(vec![(
            Method::GET,
            format!("{}?per_page=100", custom()),
            json!(
                (0..90)
                    .map(|index| json!({"hostname":format!("host{index}.example.org")}))
                    .collect::<Vec<_>>()
            ),
        )]);
        assert_eq!(
            ensure_custom_tls(&api, &settings(), "new.example.org", None)
                .unwrap_err()
                .0,
            "Included hostname capacity reached"
        );
        api.done();
    }
    #[test]
    fn custom_tls_requires_active_fallback_before_creation() {
        let api = Mock::new(vec![
            (Method::GET, format!("{}?per_page=100", custom()), json!([])),
            (
                Method::GET,
                format!("{}/fallback_origin", custom()),
                json!({"status":"pending"}),
            ),
        ]);
        assert!(
            ensure_custom_tls(&api, &settings(), "new.example.org", None)
                .unwrap_err()
                .0
                .contains("fallback origin")
        );
        api.done();
    }
    #[test]
    fn custom_tls_creation_preserves_exact_http_dv_tls_policy() {
        let api = Mock::new(vec![
            (Method::GET, format!("{}?per_page=100", custom()), json!([])),
            (
                Method::GET,
                format!("{}/fallback_origin", custom()),
                json!({"status":"active"}),
            ),
            (Method::POST, custom(), active()),
        ]);
        assert_eq!(
            ensure_custom_tls(&api, &settings(), "research.example.org", None).unwrap()["status"],
            "active"
        );
        assert_eq!(
            api.bodies.borrow()[2],
            Some(
                json!({"hostname":"research.example.org","ssl":{"method":"http","type":"dv","settings":{"min_tls_version":"1.2"}}})
            )
        );
        api.done();
    }
    #[test]
    fn provider_id_lookup_avoids_list_and_creation() {
        let api = Mock::new(vec![(
            Method::GET,
            format!("{}/provider-123", custom()),
            active(),
        )]);
        assert_eq!(
            ensure_custom_tls(
                &api,
                &settings(),
                "research.example.org",
                Some("provider-123")
            )
            .unwrap()["id"],
            "provider-123"
        );
        api.done();
    }
    #[test]
    fn mismatched_provider_id_or_route_injection_is_rejected() {
        let api = Mock::new(vec![(
            Method::GET,
            format!("{}/provider-123", custom()),
            active(),
        )]);
        assert!(
            ensure_custom_tls(&api, &settings(), "other.example.org", Some("provider-123"))
                .is_err()
        );
        api.done();
        let empty = Mock::new(vec![]);
        assert!(remove_custom_tls(&empty, &settings(), "../billing").is_err());
        assert!(ensure_custom_tls(&empty, &settings(), "http://example.org", None).is_err());
        empty.done();
    }
    #[test]
    fn tls_output_preserves_dns_records_and_requires_both_active_states() {
        let value = json!({"id":"provider-123","status":"active","ownership_verification":{"type":"txt","name":"_cf-custom-hostname.example.org","value":"exact ownership"},"ssl":{"status":"pending_validation","validation_records":[{"txt_name":"_acme-challenge.example.org","txt_value":"exact ACME"},{"http_url":"https://example.org/ignored"}]}});
        assert_eq!(
            tls_output(&value).unwrap(),
            json!({"id":"provider-123","status":"pending_tls","records":[{"type":"TXT","name":"_cf-custom-hostname.example.org","value":"exact ownership"},{"type":"TXT","name":"_acme-challenge.example.org","value":"exact ACME"}]})
        );
        assert_eq!(
            tls_output(&active()).unwrap(),
            json!({"id":"provider-123","status":"active","records":[]})
        );
    }
    #[test]
    fn custom_hostname_remove_keeps_empty_object_wire_contract() {
        let api = Mock::new(vec![(
            Method::DELETE,
            format!("{}/provider-123", custom()),
            json!({"id":"provider-123"}),
        )]);
        assert_eq!(
            remove_custom_tls(&api, &settings(), "provider-123").unwrap(),
            json!({})
        );
        api.done();
    }
    #[test]
    fn provider_failures_never_echo_authentication_material_or_response_bodies() {
        let secret = "SYNTHETIC_SECRET_TOKEN";
        let failure = parse_response(403, format!(r#"{{"success":false,"errors":[{{"code":10000,"message":"Authorization: Bearer {secret}"}}]}}"#).as_bytes()).unwrap_err();
        assert!(failure.0.contains("10000"));
        assert!(!failure.0.contains(secret));
        assert!(!failure.0.contains("Authorization"));
        assert_eq!(
            parse_response(502, secret.as_bytes()).unwrap_err().0,
            "Domain service returned HTTP 502"
        );
        assert!(parse_response(302, br#"{"success":true,"result":"redirect"}"#).is_err());
        assert_eq!(
            setup_required(),
            json!({"status":"provider_setup_required","records":[]})
        );
    }
    #[test]
    fn worker_multipart_has_legacy_metadata_module_and_framing() {
        let bytes = worker_multipart(
            b"export default {fetch(){return new Response('OK')}}",
            "fixture-boundary",
        );
        let body = String::from_utf8(bytes).unwrap();
        assert!(body.starts_with("--fixture-boundary\r\n"));
        assert!(body.ends_with("--fixture-boundary--\r\n"));
        assert!(body.contains("application/javascript+module\r\n\r\nexport default"));
        assert!(body.contains(r#""main_module":"worker.js""#));
        assert!(body.contains(r#""compatibility_date":"2026-10-29""#));
    }
    #[test]
    fn setup_key_is_private_and_repeated_setup_preserves_it() {
        let directory = tempfile::tempdir().unwrap();
        let key = directory.path().join("deployment-edge-key");
        ensure_edge_key(&key).unwrap();
        let original = fs::read_to_string(&key).unwrap();
        assert_eq!(original.len(), 64);
        assert!(original.bytes().all(|b| b.is_ascii_hexdigit()));
        ensure_edge_key(&key).unwrap();
        assert_eq!(fs::read_to_string(&key).unwrap(), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(key).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn credential_validation_does_not_expose_invalid_contents_or_file_paths() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sensitive-file-name");
        fs::write(&path, b"SYNTHETIC_SECRET").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let error = CloudflareApi::from_file(&path).err().unwrap();
        assert!(!error.0.contains("SYNTHETIC_SECRET"));
        assert!(!error.0.contains("sensitive-file-name"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(
                CloudflareApi::from_file(&path)
                    .err()
                    .unwrap()
                    .0
                    .contains("600")
            );
            let alias = directory.path().join("alias");
            symlink(path, &alias).unwrap();
            assert!(CloudflareApi::from_file(&alias).is_err());
        }
    }
    #[test]
    #[cfg(unix)]
    fn provider_capacity_checks_are_serialized_and_lock_released_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("private")).unwrap();
        let first = provider_lock(directory.path()).unwrap();
        let other = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.path().join("private/custom-hostnames.lock"))
            .unwrap();
        assert!(
            rustix::fs::flock(&other, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                .is_err()
        );
        drop(first);
        assert!(
            rustix::fs::flock(&other, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_ok()
        );
    }
}
