//! Cloudflare routing management. Default is a read-only plan; writes require --apply.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
pub type Result<T> = std::result::Result<T, String>;
pub const ACCOUNT: &str = "3a4fa468d49633b9a24c2c40539ea8a5";
pub const ZONE: &str = "99d6ebc0a8271e9b2104245b25c47529";
pub const TUNNEL: &str = "3e478ed8-6349-443f-adb0-a446ae6a82fa";
pub fn target() -> String {
    format!("{TUNNEL}.cfargotunnel.com")
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Site {
    Careers,
    Developers,
    Manufacturing,
    EquipmentPortals,
    Deployments,
    Latex,
    Workspaces,
    ProjectNamespaces,
}
impl Site {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "careers" => Ok(Self::Careers),
            "developers" => Ok(Self::Developers),
            "manufacturing" => Ok(Self::Manufacturing),
            "equipment-portals" => Ok(Self::EquipmentPortals),
            "deployments" => Ok(Self::Deployments),
            "latex" => Ok(Self::Latex),
            "workspaces" => Ok(Self::Workspaces),
            "project-namespaces" => Ok(Self::ProjectNamespaces),
            _ => Err("Unknown routing command".into()),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Careers => "careers",
            Self::Developers => "developers",
            Self::Manufacturing => "manufacturing",
            Self::EquipmentPortals => "equipment-portals",
            Self::Deployments => "deployments",
            Self::Latex => "latex",
            Self::Workspaces => "workspaces",
            Self::ProjectNamespaces => "project-namespaces",
        }
    }
    pub fn hosts(self) -> Vec<&'static str> {
        match self {
            Self::Careers => vec!["careers.cbgpu.com"],
            Self::Developers => vec!["developers.cbgpu.com"],
            Self::Manufacturing | Self::EquipmentPortals => vec![
                "print.cbgpu.com",
                "laser.cbgpu.com",
                "manufacturing.cbgpu.com",
            ],
            Self::Deployments => vec!["*.cbgpu.com"],
            Self::Latex => vec!["latex.cbgpu.com"],
            Self::Workspaces => vec![
                "student.cbgpu.com",
                "researcher.cbgpu.com",
                "developer.cbgpu.com",
                "app.cbgpu.com",
            ],
            Self::ProjectNamespaces => vec!["*.front.cbgpu.com", "*.server.cbgpu.com"],
        }
    }
    pub fn service(self) -> &'static str {
        match self {
            Self::Careers => "http://127.0.0.1:3530",
            Self::Developers | Self::Manufacturing | Self::EquipmentPortals => {
                "http://127.0.0.1:3400"
            }
            Self::Deployments | Self::ProjectNamespaces => "http://127.0.0.1:3510",
            Self::Latex => "http://127.0.0.1:3520",
            Self::Workspaces => "http://127.0.0.1:3000",
        }
    }
}
pub trait Api {
    fn call(&mut self, route: &str, method: &str, body: Option<&Value>) -> Result<Value>;
}
#[derive(Clone)]
pub enum Credential {
    Token(String),
    Cookies(String),
}
pub fn read_token(path: &Path) -> Result<String> {
    let meta = fs::metadata(path).map_err(|_| "Routing token file unavailable")?;
    if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
        return Err("Routing token must be a private file with permissions600".into());
    }
    let value = fs::read_to_string(path).map_err(|_| "Cannot read private routing token")?;
    let value = value.trim();
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err("Routing token is empty or malformed".into());
    }
    Ok(value.into())
}
pub fn browser_cookies(path: &Path) -> Result<String> {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "Readonly Cloudflare cookie database unavailable")?;
    let mut query=db.prepare("select name,value from moz_cookies where host in ('.cloudflare.com','dash.cloudflare.com','.dash.cloudflare.com')").map_err(|_|"Cannot read Cloudflare browser session")?;
    let rows = query
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| "Cannot query browser session")?;
    let mut values = Vec::new();
    for row in rows {
        let (name, value) = row.map_err(|_| "Cannot read browser cookie")?;
        if name.contains(['\n', '\r', ';', '=']) || value.contains(['\n', '\r', ';']) {
            return Err("Malformed browser session cookie".into());
        }
        values.push(format!("{name}={value}"));
    }
    if values.is_empty() {
        return Err("No Cloudflare browser session; configure scoped token explicitly".into());
    }
    Ok(values.join("; "))
}
/// Preserve former precedence: developers fixed token; careers env override then
/// cookie fallback; other routes cookie transport. Explicit CLI token overrides all.
pub fn credentials(
    site: Site,
    explicit: Option<&Path>,
    careers_override: Option<&Path>,
    fixed: &Path,
    cookies: &Path,
) -> Result<Credential> {
    if let Some(path) = explicit {
        return read_token(path).map(Credential::Token);
    }
    if site == Site::Developers {
        return read_token(fixed).map(Credential::Token);
    }
    if site == Site::Careers {
        let path = careers_override.unwrap_or(fixed);
        if path.exists() {
            return read_token(path).map(Credential::Token);
        }
    }
    browser_cookies(cookies).map(Credential::Cookies)
}
pub struct Cloudflare {
    credential: Credential,
    client: reqwest::blocking::Client,
}
impl Cloudflare {
    pub fn new(credential: Credential) -> Result<Self> {
        Ok(Self {
            credential,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "Cannot initialize Cloudflare client")?,
        })
    }
}
impl Api for Cloudflare {
    fn call(&mut self, route: &str, method: &str, body: Option<&Value>) -> Result<Value> {
        if !["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&method)
            || route.starts_with('/')
            || route.contains(['\n', '\r'])
        {
            return Err("Invalid Cloudflare request".into());
        }
        let base = match self.credential {
            Credential::Token(_) => "https://api.cloudflare.com/client/v4/",
            Credential::Cookies(_) => "https://dash.cloudflare.com/api/v4/",
        };
        let mut request = self
            .client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| "Invalid API method")?,
                format!("{base}{route}"),
            )
            .header("User-Agent", "CBGPU routing setup");
        request = match &self.credential {
            Credential::Token(token) => request.bearer_auth(token),
            Credential::Cookies(cookie) => {
                let request = request
                    .header("Cookie", cookie)
                    .header("Referer", "https://dash.cloudflare.com/");
                if body.is_some() {
                    request.header("X-Cross-Site-Security", "dash")
                } else {
                    request
                }
            }
        };
        if let Some(body) = body {
            request = request.json(body)
        }
        let response = request
            .send()
            .map_err(|_| "Cloudflare network request failed; credentials omitted")?;
        if !response.status().is_success() {
            return Err(format!(
                "Cloudflare rejected routing access(HTTP{}); check scoped DNS/Tunnel permissions",
                response.status().as_u16()
            ));
        }
        let value: Value = response
            .json()
            .map_err(|_| "Cloudflare returned a non-JSON response")?;
        if value["success"] != true {
            return Err("Cloudflare rejected operation; credentials omitted".into());
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| "Cloudflare result missing".into())
    }
}
fn id(value: &Value) -> Result<&str> {
    let value = value.as_str().ok_or("Missing Cloudflare object ID")?;
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("Invalid Cloudflare object ID".into());
    }
    Ok(value)
}
fn dns_query(zone: &str, host: &str) -> String {
    let encoded: String =
        reqwest::Url::parse_with_params("https://cloudflare.invalid/", &[("name", host)])
            .expect("constant URL")
            .query()
            .unwrap()
            .into();
    format!("zones/{zone}/dns_records?{encoded}")
}
fn records(value: Value) -> Result<Vec<Value>> {
    value
        .as_array()
        .cloned()
        .ok_or_else(|| "Invalid Cloudflare DNS list".into())
}
fn validate_dns(rows: &[Value]) -> Result<()> {
    if rows.len() > 1 {
        return Err("Duplicate DNS records conflict; left unchanged".into());
    }
    if rows.iter().any(|r| {
        r["type"] != "CNAME"
            || r["content"].as_str().unwrap_or("").trim_end_matches('.') != target()
    }) {
        return Err("Conflicting DNS record left unchanged".into());
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxtRecord {
    pub name: String,
    pub content: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub site: Site,
    pub endpoint: String,
    pub zone: String,
    pub before: Value,
    pub config: Value,
    pub dns: BTreeMap<String, Vec<Value>>,
    pub broad_dns: Vec<Value>,
    pub validation_txt: Vec<TxtRecord>,
}
impl Plan {
    pub fn summary(&self, apply: bool) -> Value {
        json!({"mode":if apply{"apply"}else{"check"},"command":self.site,"hostnames":self.site.hosts(),"service":self.site.service(),"dns_target":target(),"tunnel_changed":self.before["config"]!=self.config,"existing_routes":self.before["config"]["ingress"].as_array().map(Vec::len),"remove_owned_broad_wildcard_dns":self.broad_dns.iter().filter(|r|r["type"]=="CNAME"&&r["content"].as_str().unwrap_or("").trim_end_matches('.')==target()).count(),"validation_txt_records":self.validation_txt.len()})
    }
}

pub fn edit_config(
    site: Site,
    before: &Value,
    dns: &BTreeMap<String, Vec<Value>>,
) -> Result<Value> {
    let mut config = before["config"].clone();
    let rules = config["ingress"]
        .as_array_mut()
        .ok_or("Tunnel ingress missing")?;
    if rules.is_empty()
        || rules
            .last()
            .and_then(|r| r.get("hostname"))
            .is_some_and(|v| !v.is_null())
    {
        return Err("Terminal tunnel catch-all missing".into());
    }
    if rules[..rules.len() - 1]
        .iter()
        .any(|r| r.get("hostname").is_none() || r["hostname"].is_null())
    {
        return Err("Non-terminal catch-all would shadow applications".into());
    }
    if site == Site::ProjectNamespaces {
        for rule in rules.iter().filter(|r| r["hostname"] == "*.cbgpu.com") {
            if rule["service"] != "http://127.0.0.1:3510"
                || rule.get("path").is_some_and(|p| !p.is_null() && p != "")
            {
                return Err("Broad wildcard belongs to another application".into());
            }
        }
        rules.retain(|r| r["hostname"] != "*.cbgpu.com");
    }
    for host in site.hosts() {
        validate_dns(dns.get(host).ok_or("Missing DNS snapshot")?)?;
        let matching: Vec<_> = rules
            .iter()
            .enumerate()
            .filter(|(_, r)| r["hostname"] == host)
            .map(|(i, _)| i)
            .collect();
        if matching.len() > 1 {
            return Err("Duplicate hostname routes conflict; left unchanged".into());
        }
        if let Some(index) = matching.first() {
            let rule = &mut rules[*index];
            if rule.get("path").is_some_and(|p| !p.is_null() && p != "") {
                return Err("Path-scoped route conflicts; left unchanged".into());
            }
            let previous = rule["service"].as_str().ok_or("Route service missing")?;
            let allowed = previous == site.service()
                || site == Site::Developers
                || host == "manufacturing.cbgpu.com" && previous == "http://127.0.0.1:3000";
            if !allowed {
                return Err("Existing tunnel route conflicts; left unchanged".into());
            }
            rule["service"] = json!(site.service());
        } else {
            let desired = json!({"hostname":host,"service":site.service()});
            if site == Site::Developers {
                rules.insert(0, desired)
            } else {
                rules.insert(rules.len() - 1, desired)
            }
        }
    }
    // Specific hostnames must precede the broad wildcard. Never reorder unrelated rules.
    if let Some(wildcard) = rules.iter().position(|r| r["hostname"] == "*.cbgpu.com")
        && rules[wildcard + 1..]
            .iter()
            .any(|r| r.get("hostname").is_some_and(|v| !v.is_null()))
    {
        return Err("Wildcard would shadow an existing application".into());
    }
    Ok(config)
}
pub fn plan(api: &mut impl Api, site: Site) -> Result<Plan> {
    let (mut account, mut zone) = (ACCOUNT.to_owned(), ZONE.to_owned());
    if site == Site::Developers {
        let zones = records(api.call("zones?name=cbgpu.com", "GET", None)?)?;
        let zones: Vec<_> = zones.iter().filter(|z| z["name"] == "cbgpu.com").collect();
        if zones.len() != 1 {
            return Err("Token must access exactly the cbgpu.com DNS zone".into());
        }
        zone = id(&zones[0]["id"])?.into();
        account = id(&zones[0]["account"]["id"])?.into();
    }
    let endpoint = format!("accounts/{account}/cfd_tunnel/{TUNNEL}/configurations");
    let before = api.call(&endpoint, "GET", None)?;
    if before.get("version").is_none_or(Value::is_null) {
        return Err("Tunnel version missing; cannot safely compare edits".into());
    }
    let mut dns = BTreeMap::new();
    for host in site.hosts() {
        dns.insert(
            host.to_owned(),
            records(api.call(&dns_query(&zone, host), "GET", None)?)?,
        );
    }
    let config = edit_config(site, &before, &dns)?;
    let mut broad_dns = vec![];
    let mut validation_txt = vec![];
    if site == Site::ProjectNamespaces {
        broad_dns = records(api.call(&dns_query(&zone, "*.cbgpu.com"), "GET", None)?)?;
        let packs = records(api.call(
            &format!("zones/{zone}/ssl/certificate_packs?status=all"),
            "GET",
            None,
        )?)?;
        for pack in packs {
            if pack["type"] != "advanced"
                || !pack["hosts"].as_array().is_some_and(|hosts| {
                    hosts
                        .iter()
                        .any(|h| h == "front.cbgpu.com" || h == "server.cbgpu.com")
                })
            {
                continue;
            }
            let detail = api.call(
                &format!("zones/{zone}/ssl/certificate_packs/{}", id(&pack["id"])?),
                "GET",
                None,
            )?;
            for record in detail["validation_records"].as_array().unwrap_or(&vec![]) {
                if record["status"] != "pending" {
                    continue;
                }
                let Some(name) = record["txt_name"].as_str().filter(|n| {
                    [
                        "_acme-challenge.front.cbgpu.com",
                        "_acme-challenge.server.cbgpu.com",
                    ]
                    .contains(n)
                }) else {
                    continue;
                };
                let content = record["txt_value"]
                    .as_str()
                    .ok_or("Certificate validation value missing")?;
                let current = records(api.call(
                    &format!("{}&type=TXT", dns_query(&zone, name)),
                    "GET",
                    None,
                )?)?;
                if !current.iter().any(|r| r["content"] == content) {
                    validation_txt.push(TxtRecord {
                        name: name.into(),
                        content: content.into(),
                    });
                }
            }
        }
    }
    Ok(Plan {
        site,
        endpoint,
        zone,
        before,
        config,
        dns,
        broad_dns,
        validation_txt,
    })
}
fn private_backup(root: &Path, plan: &Plan) -> Result<PathBuf> {
    let dir = root.join("private");
    fs::create_dir_all(&dir).map_err(|_| "Cannot create private routing backup directory")?;
    let path = dir.join(format!(
        "cloudflare-{}-{}-{}.json",
        plan.site.label(),
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| "Cannot create exclusive private routing backup")?;
    serde_json::to_writer(&mut file, plan).map_err(|_| "Cannot serialize routing snapshot")?;
    file.write_all(b"\n")
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot persist routing snapshot")?;
    fs::File::open(&dir)
        .and_then(|f| f.sync_all())
        .map_err(|_| "Cannot sync routing backup directory")?;
    Ok(path)
}
pub fn apply(api: &mut impl Api, plan: &Plan, root: &Path) -> Result<PathBuf> {
    let current = api.call(&plan.endpoint, "GET", None)?;
    if current["version"] != plan.before["version"] || current["config"] != plan.before["config"] {
        return Err("Tunnel changed concurrently; review again".into());
    }
    for (host, snapshot) in &plan.dns {
        if records(api.call(&dns_query(&plan.zone, host), "GET", None)?)? != *snapshot {
            return Err("DNS changed concurrently; review again".into());
        }
    }
    if plan.site == Site::ProjectNamespaces
        && records(api.call(&dns_query(&plan.zone, "*.cbgpu.com"), "GET", None)?)? != plan.broad_dns
    {
        return Err("Broad wildcard DNS changed concurrently".into());
    }
    let backup = private_backup(root, plan)?;
    let changed = plan.config != plan.before["config"];
    let result: Result<()> = (|| {
        if changed {
            api.call(&plan.endpoint, "PUT", Some(&json!({"config":plan.config})))?;
        }
        let dns = format!("zones/{}/dns_records", plan.zone);
        for (host, existing) in &plan.dns {
            if existing.is_empty() {
                api.call(&dns,"POST",Some(&json!({"type":"CNAME","name":host,"content":target(),"proxied":true,"ttl":1})))?;
            } else if existing[0]["proxied"] != true {
                api.call(
                    &format!("{dns}/{}", id(&existing[0]["id"])?),
                    "PATCH",
                    Some(&json!({"proxied":true})),
                )?;
            }
        }
        for record in &plan.broad_dns {
            if record["type"] == "CNAME"
                && record["content"]
                    .as_str()
                    .unwrap_or("")
                    .trim_end_matches('.')
                    == target()
            {
                api.call(&format!("{dns}/{}", id(&record["id"])?), "DELETE", None)?;
            }
        }
        for record in &plan.validation_txt {
            let now = records(api.call(
                &format!("{}&type=TXT", dns_query(&plan.zone, &record.name)),
                "GET",
                None,
            )?)?;
            if !now.iter().any(|r| r["content"] == record.content) {
                api.call(
                    &dns,
                    "POST",
                    Some(
                        &json!({"type":"TXT","name":record.name,"content":record.content,"ttl":60}),
                    ),
                )?;
            }
        }
        let final_state = api.call(&plan.endpoint, "GET", None)?;
        if final_state["config"] != plan.config {
            return Err(
                "Tunnel verification failed; external edits will not be overwritten".into(),
            );
        }
        for host in plan.site.hosts() {
            let rows = records(api.call(&dns_query(&plan.zone, host), "GET", None)?)?;
            validate_dns(&rows)?;
            if rows.len() != 1 || rows[0]["proxied"] != true {
                return Err("DNS verification failed".into());
            }
        }
        if plan.site == Site::ProjectNamespaces {
            let broad = records(api.call(&dns_query(&plan.zone, "*.cbgpu.com"), "GET", None)?)?;
            if broad.iter().any(|r| {
                r["type"] == "CNAME"
                    && r["content"].as_str().unwrap_or("").trim_end_matches('.') == target()
            }) {
                return Err("Owned broad wildcard DNS still exists".into());
            }
        }
        for record in &plan.validation_txt {
            let now = records(api.call(
                &format!("{}&type=TXT", dns_query(&plan.zone, &record.name)),
                "GET",
                None,
            )?)?;
            if !now.iter().any(|r| r["content"] == record.content) {
                return Err("Certificate validation TXT verification failed".into());
            }
        }
        Ok(())
    })();
    if let Err(problem) = result {
        if changed {
            match api.call(&plan.endpoint, "GET", None) {
                Ok(now) if now["config"] == plan.config => {
                    if api
                        .call(
                            &plan.endpoint,
                            "PUT",
                            Some(&json!({"config":plan.before["config"]})),
                        )
                        .is_err()
                    {
                        return Err("Routing failed and tunnel restoration failed; private snapshot retained".into());
                    }
                }
                _ => return Err(
                    "Routing failed; concurrent tunnel edit preserved; private snapshot retained"
                        .into(),
                ),
            }
        }
        return Err(format!(
            "{problem}; tunnel restored when safe, DNS snapshot retained for explicit recovery"
        ));
    }
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    fn base() -> Value {
        json!({"version":1,"config":{"warp-routing":{"enabled":true},"originRequest":{"noTLSVerify":false,"httpHostHeader":"preserved"},"ingress":[{"hostname":"cbgpu.com","service":"http://127.0.0.1:3000","originRequest":{"connectTimeout":30}},{"service":"http_status:404"}]}})
    }
    fn snapshot(site: Site) -> BTreeMap<String, Vec<Value>> {
        site.hosts()
            .into_iter()
            .map(|h| (h.into(), vec![]))
            .collect()
    }
    #[test]
    fn all_commands_preserve_unrelated_routes_and_proxy_settings() {
        for site in [
            Site::Careers,
            Site::Developers,
            Site::Manufacturing,
            Site::EquipmentPortals,
            Site::Deployments,
            Site::Latex,
            Site::Workspaces,
            Site::ProjectNamespaces,
        ] {
            let before = base();
            let config = edit_config(site, &before, &snapshot(site)).unwrap();
            assert_eq!(config["originRequest"], before["config"]["originRequest"]);
            assert_eq!(config["warp-routing"], before["config"]["warp-routing"]);
            assert!(
                config["ingress"]
                    .as_array()
                    .unwrap()
                    .contains(&before["config"]["ingress"][0])
            );
            for host in site.hosts() {
                assert!(
                    config["ingress"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|r| r["hostname"] == host && r["service"] == site.service())
                )
            }
        }
    }
    #[test]
    fn conflicts_missing_catchall_and_duplicates_fail_closed() {
        let mut before = base();
        before["config"]["ingress"].as_array_mut().unwrap().insert(
            0,
            json!({"hostname":"careers.cbgpu.com","service":"http://other"}),
        );
        assert!(edit_config(Site::Careers, &before, &snapshot(Site::Careers)).is_err());
        before = base();
        before["config"]["ingress"].as_array_mut().unwrap().pop();
        assert!(edit_config(Site::Careers, &before, &snapshot(Site::Careers)).is_err());
        let mut dns = snapshot(Site::Careers);
        dns.insert(
            "careers.cbgpu.com".into(),
            vec![json!({"type":"A","content":"1.2.3.4"})],
        );
        assert!(edit_config(Site::Careers, &base(), &dns).is_err());
    }
    #[test]
    fn manufacturing_alias_and_legacy_service_upgrade_preserve_rule_options() {
        let mut before = base();
        before["config"]["ingress"].as_array_mut().unwrap().insert(0,json!({"hostname":"manufacturing.cbgpu.com","service":"http://127.0.0.1:3000","originRequest":{"httpHostHeader":"manufacturing.internal"}}));
        let config =
            edit_config(Site::Manufacturing, &before, &snapshot(Site::Manufacturing)).unwrap();
        assert_eq!(
            config["ingress"][0]["originRequest"],
            before["config"]["ingress"][0]["originRequest"]
        );
        assert_eq!(
            config,
            edit_config(
                Site::EquipmentPortals,
                &before,
                &snapshot(Site::EquipmentPortals)
            )
            .unwrap()
        );
    }
    #[test]
    fn developers_service_update_keeps_proxy_options_and_rejects_path_conflicts() {
        let mut before = base();
        before["config"]["ingress"].as_array_mut().unwrap().insert(0,json!({"hostname":"developers.cbgpu.com","service":"http://old","originRequest":{"noTLSVerify":true}}));
        let config = edit_config(Site::Developers, &before, &snapshot(Site::Developers)).unwrap();
        assert_eq!(
            config["ingress"][0]["originRequest"],
            before["config"]["ingress"][0]["originRequest"]
        );
        before["config"]["ingress"][0]["path"] = json!("/api");
        assert!(edit_config(Site::Developers, &before, &snapshot(Site::Developers)).is_err());
    }
    #[test]
    fn namespace_replacement_only_removes_owned_broad_wildcard() {
        let mut before = base();
        before["config"]["ingress"].as_array_mut().unwrap().insert(
            1,
            json!({"hostname":"*.cbgpu.com","service":"http://127.0.0.1:3510"}),
        );
        let config = edit_config(
            Site::ProjectNamespaces,
            &before,
            &snapshot(Site::ProjectNamespaces),
        )
        .unwrap();
        assert!(
            !config["ingress"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["hostname"] == "*.cbgpu.com")
        );
        before["config"]["ingress"][1]["service"] = json!("http://other");
        assert!(
            edit_config(
                Site::ProjectNamespaces,
                &before,
                &snapshot(Site::ProjectNamespaces)
            )
            .is_err()
        );
    }
    #[derive(Default)]
    struct FakeApi {
        state: Value,
        dns: BTreeMap<String, Vec<Value>>,
        calls: Vec<(String, String, Option<Value>)>,
        fail_dns: bool,
        concurrent: bool,
        packs: Value,
        pack_details: Value,
    }
    impl FakeApi {
        fn fixture() -> Self {
            Self {
                state: base(),
                ..Self::default()
            }
        }
        fn writes(&self) -> usize {
            self.calls.iter().filter(|(_, m, _)| m != "GET").count()
        }
    }
    impl Api for FakeApi {
        fn call(&mut self, route: &str, method: &str, body: Option<&Value>) -> Result<Value> {
            self.calls
                .push((route.into(), method.into(), body.cloned()));
            if route == "zones?name=cbgpu.com" {
                return Ok(json!([{"name":"cbgpu.com","id":ZONE,"account":{"id":ACCOUNT}}]));
            }
            if route.ends_with("/configurations") {
                if method == "PUT" {
                    self.state["config"] = body.unwrap()["config"].clone();
                    let version = self.state["version"].as_u64().unwrap() + 1;
                    self.state["version"] = json!(version);
                }
                return Ok(self.state.clone());
            }
            if route.contains("certificate_packs?") {
                return Ok(if self.packs.is_null() {
                    json!([])
                } else {
                    self.packs.clone()
                });
            }
            if route.contains("certificate_packs/") {
                return Ok(self.pack_details.clone());
            }
            if route.contains("dns_records?") {
                let url = reqwest::Url::parse(&format!("https://fake/{route}")).unwrap();
                let host = url
                    .query_pairs()
                    .find(|(k, _)| k == "name")
                    .unwrap()
                    .1
                    .into_owned();
                return Ok(json!(self.dns.get(&host).cloned().unwrap_or_default()));
            }
            if route.ends_with("/dns_records") && method == "POST" {
                if self.fail_dns {
                    if self.concurrent {
                        self.state["config"]["operator-edit"] = json!(true)
                    }
                    return Err("synthetic DNS failure".into());
                }
                let mut record = body.unwrap().clone();
                record["id"] = json!("newrecord");
                let name = record["name"].as_str().unwrap().to_owned();
                self.dns.entry(name).or_default().push(record);
                return Ok(json!({"id":"newrecord"}));
            }
            if method == "PATCH" {
                for rows in self.dns.values_mut() {
                    for row in rows {
                        if row["id"].as_str().is_some_and(|id| route.ends_with(id)) {
                            row["proxied"] = json!(true)
                        }
                    }
                }
                return Ok(json!({}));
            }
            if method == "DELETE" {
                for rows in self.dns.values_mut() {
                    rows.retain(|r| !r["id"].as_str().is_some_and(|id| route.ends_with(id)));
                }
                return Ok(json!({}));
            }
            Err("Unexpected fake API operation".into())
        }
    }
    #[test]
    fn dry_plans_make_only_gets_and_no_backups() {
        for site in [
            Site::Careers,
            Site::Developers,
            Site::Manufacturing,
            Site::EquipmentPortals,
            Site::Deployments,
            Site::Latex,
            Site::Workspaces,
            Site::ProjectNamespaces,
        ] {
            let mut api = FakeApi::fixture();
            let plan = crate::plan(&mut api, site).unwrap();
            assert_eq!(api.writes(), 0);
            assert_eq!(plan.summary(false)["mode"], "check");
        }
    }
    #[test]
    fn apply_snapshots_before_writing_and_verifies_routes_dns() {
        let tmp = tempdir().unwrap();
        let mut api = FakeApi::fixture();
        let plan = crate::plan(&mut api, Site::Workspaces).unwrap();
        let backup = apply(&mut api, &plan, tmp.path()).unwrap();
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let saved: Plan = serde_json::from_slice(&fs::read(backup).unwrap()).unwrap();
        assert_eq!(saved.before, base());
        assert_eq!(api.state["config"], plan.config);
        assert_eq!(api.dns.len(), 4);
    }
    #[test]
    fn concurrent_tunnel_or_dns_change_prevents_all_writes() {
        let tmp = tempdir().unwrap();
        let mut api = FakeApi::fixture();
        let plan = crate::plan(&mut api, Site::Careers).unwrap();
        api.state["version"] = json!(2);
        assert!(apply(&mut api, &plan, tmp.path()).is_err());
        assert_eq!(api.writes(), 0);
        api.state = base();
        api.dns.insert(
            "careers.cbgpu.com".into(),
            vec![json!({"id":"foreign","type":"A"})],
        );
        assert!(apply(&mut api, &plan, tmp.path()).is_err());
        assert_eq!(api.writes(), 0);
    }
    #[test]
    fn dns_failure_restores_only_our_tunnel_change() {
        let tmp = tempdir().unwrap();
        let mut api = FakeApi::fixture();
        let plan = crate::plan(&mut api, Site::Careers).unwrap();
        api.fail_dns = true;
        assert!(apply(&mut api, &plan, tmp.path()).is_err());
        assert_eq!(api.state["config"], base()["config"]);
        let mut api = FakeApi::fixture();
        let plan = crate::plan(&mut api, Site::Careers).unwrap();
        api.fail_dns = true;
        api.concurrent = true;
        assert!(apply(&mut api, &plan, tmp.path()).is_err());
        assert_eq!(api.state["config"]["operator-edit"], true);
    }
    #[test]
    fn existing_matching_dns_is_proxied_without_replacing_record() {
        let tmp = tempdir().unwrap();
        let mut api = FakeApi::fixture();
        api.dns.insert("latex.cbgpu.com".into(),vec![json!({"id":"original","type":"CNAME","content":format!("{}.",target()),"proxied":false,"ttl":120})]);
        let plan = crate::plan(&mut api, Site::Latex).unwrap();
        apply(&mut api, &plan, tmp.path()).unwrap();
        assert_eq!(api.dns["latex.cbgpu.com"][0]["id"], "original");
        assert_eq!(api.dns["latex.cbgpu.com"][0]["ttl"], 120);
        assert_eq!(api.dns["latex.cbgpu.com"][0]["proxied"], true);
    }
    #[test]
    fn token_precedence_private_mode_and_cookie_database_are_preserved() {
        let tmp = tempdir().unwrap();
        let fixed = tmp.path().join("fixed");
        let override_path = tmp.path().join("override");
        for (path, value) in [
            (&fixed, "fixed-secret"),
            (&override_path, "override-secret"),
        ] {
            fs::write(path, value).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let cookies = tmp.path().join("cookies.sqlite");
        let db = rusqlite::Connection::open(&cookies).unwrap();
        db.execute_batch("create table moz_cookies(name text,value text,host text);insert into moz_cookies values('cf_session','synthetic-cookie','.cloudflare.com');").unwrap();
        let before = fs::read(&cookies).unwrap();
        assert!(
            matches!(credentials(Site::Developers,None,Some(&override_path),&fixed,&cookies).unwrap(),Credential::Token(s)if s=="fixed-secret")
        );
        assert!(
            matches!(credentials(Site::Careers,None,Some(&override_path),&fixed,&cookies).unwrap(),Credential::Token(s)if s=="override-secret")
        );
        assert!(matches!(
            credentials(
                Site::Workspaces,
                None,
                Some(&override_path),
                &fixed,
                &cookies
            )
            .unwrap(),
            Credential::Cookies(_)
        ));
        assert!(matches!(
            credentials(Site::Workspaces, Some(&fixed), None, &fixed, &cookies).unwrap(),
            Credential::Token(_)
        ));
        fs::set_permissions(&override_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(credentials(Site::Careers, None, Some(&override_path), &fixed, &cookies).is_err());
        assert_eq!(fs::read(cookies).unwrap(), before);
    }
    #[test]
    fn namespace_certificate_validation_and_owned_wildcard_cleanup() {
        let tmp = tempdir().unwrap();
        let mut api = FakeApi::fixture();
        api.state["config"]["ingress"]
            .as_array_mut()
            .unwrap()
            .insert(
                1,
                json!({"hostname":"*.cbgpu.com","service":"http://127.0.0.1:3510"}),
            );
        api.dns.insert(
            "*.cbgpu.com".into(),
            vec![
                json!({"id":"owned","type":"CNAME","content":target()}),
                json!({"id":"unrelated","type":"TXT","content":"keep"}),
            ],
        );
        api.packs = json!([{"id":"pack","type":"advanced","hosts":["front.cbgpu.com"]}]);
        api.pack_details = json!({"validation_records":[{"status":"pending","txt_name":"_acme-challenge.front.cbgpu.com","txt_value":"synthetic-proof"},{"status":"pending","txt_name":"other.cbgpu.com","txt_value":"must-not-create"}]});
        let plan = crate::plan(&mut api, Site::ProjectNamespaces).unwrap();
        assert_eq!(plan.validation_txt.len(), 1);
        apply(&mut api, &plan, tmp.path()).unwrap();
        assert_eq!(api.dns["*.cbgpu.com"].len(), 1);
        assert_eq!(api.dns["*.cbgpu.com"][0]["id"], "unrelated");
        assert_eq!(
            api.dns["_acme-challenge.front.cbgpu.com"][0]["content"],
            "synthetic-proof"
        );
        assert!(!api.dns.contains_key("other.cbgpu.com"));
    }
    #[test]
    fn duplicate_and_shadowing_rules_are_rejected() {
        let mut before = base();
        for _ in 0..2 {
            before["config"]["ingress"].as_array_mut().unwrap().insert(
                0,
                json!({"hostname":"careers.cbgpu.com","service":Site::Careers.service()}),
            );
        }
        assert!(edit_config(Site::Careers, &before, &snapshot(Site::Careers)).is_err());
        let mut before = base();
        before["config"]["ingress"].as_array_mut().unwrap().insert(
            0,
            json!({"hostname":"*.cbgpu.com","service":"http://127.0.0.1:3510"}),
        );
        assert!(edit_config(Site::Deployments, &before, &snapshot(Site::Deployments)).is_err());
    }
}
