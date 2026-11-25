use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};
pub type Result<T> = std::result::Result<T, String>;
fn error(message: &str) -> String {
    message.into()
}
fn present(value: &Value) -> bool {
    !value.is_null()
        && value != false
        && value != ""
        && value.as_array().is_none_or(|v| !v.is_empty())
        && value.as_object().is_none_or(|v| !v.is_empty())
}
pub fn within(root: &Path, name: &str) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(|_| error("Repository root is unavailable"))?;
    let joined = if Path::new(name).is_absolute() {
        PathBuf::from(name)
    } else {
        root.join(name)
    };
    let mut value = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                value.pop();
            }
            Component::CurDir => {}
            part => value.push(part.as_os_str()),
        }
    }
    let mut missing = Vec::new();
    let mut ancestor = value.as_path();
    while !ancestor.exists() {
        if std::fs::symlink_metadata(ancestor).is_ok() {
            return Err(error("Invalid repository symlink"));
        }
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| error("Invalid filesystem path"))?
                .to_owned(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| error("Invalid filesystem path"))?;
    }
    let mut resolved = ancestor
        .canonicalize()
        .map_err(|_| error("Cannot resolve repository path"))?;
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    if !resolved.starts_with(root) {
        return Err(error(
            "Compose files and build contexts must stay inside the repository",
        ));
    }
    Ok(resolved)
}
pub fn expand(value: &str, env: &BTreeMap<String, String>) -> Result<String> {
    let regex = regex::Regex::new(r"\$\{([A-Za-z_]\w*)(?:(:-|-)([^}]*))?\}").expect("fixed regex");
    let value = regex
        .replace_all(value, |cap: &regex::Captures<'_>| {
            env.get(&cap[1])
                .cloned()
                .unwrap_or_else(|| cap.get(3).map_or("", |v| v.as_str()).to_owned())
        })
        .into_owned();
    if value.contains('$') {
        return Err(error("Unresolved variable in a filesystem path"));
    }
    Ok(value)
}
fn string(value: &Value) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| error("Expected a string setting"))
}
pub fn parse(text: &str) -> Result<Value> {
    let mut raw: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|_| error("Invalid Compose YAML"))?;
    raw.apply_merge()
        .map_err(|_| error("Invalid Compose YAML merge"))?;
    serde_json::to_value(raw).map_err(|_| error("Compose keys must be strings"))
}
pub fn validate(
    raw: &mut Value,
    root: &Path,
    env: &BTreeMap<String, String>,
    socket: &Path,
) -> Result<()> {
    if !raw.is_object() || present(&raw["include"]) {
        return Err(error("Compose include is not supported"));
    }
    let services = raw["services"]
        .as_object_mut()
        .ok_or_else(|| error("Missing Compose services"))?;
    if !(1..=20).contains(&services.len()) {
        return Err(error("Use 1–20 Compose services"));
    }
    let valid_name = regex::Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]{0,60}$").expect("fixed regex");
    for (name, service) in services {
        if !valid_name.is_match(name) || !service.is_object() {
            return Err(error("Invalid service name"));
        }
        for key in [
            "privileged",
            "devices",
            "device_cgroup_rules",
            "gpus",
            "volumes_from",
            "pid",
            "ipc",
            "userns_mode",
            "cgroup_parent",
            "cgroup",
            "uts",
            "use_api_socket",
            "post_start",
            "pre_stop",
            "develop",
            "provider",
            "credential_spec",
            "extends",
            "external_links",
            "extra_hosts",
            "network_mode",
            "sysctls",
        ] {
            if present(&service[key]) {
                return Err(format!("Unsafe Compose setting: {key}"));
            }
        }
        if !service["runtime"].is_null() && service["runtime"] != "runc" {
            return Err(error("Custom/GPU runtimes are not allowed"));
        }
        for (key, allowed) in [
            ("cap_add", &["SYS_NICE", "NET_BIND_SERVICE"][..]),
            (
                "security_opt",
                &[
                    "no-new-privileges:true",
                    "no-new-privileges=true",
                    "label=disable",
                ][..],
            ),
        ] {
            if !service[key].is_null() {
                for v in service[key]
                    .as_array()
                    .ok_or_else(|| error("Expected a list"))?
                {
                    let value = string(v)?;
                    let cap = value.to_uppercase();
                    if !allowed.contains(&if key == "cap_add" {
                        cap.as_str()
                    } else {
                        value
                    }) {
                        return Err(error("Unsafe container capability or security option"));
                    }
                }
            }
        }
        if present(&service["env_file"]) {
            let files = if let Some(files) = service["env_file"].as_array() {
                files.clone()
            } else {
                vec![service["env_file"].clone()]
            };
            for file in files {
                let file = if file.is_object() {
                    &file["path"]
                } else {
                    &file
                };
                within(root, &expand(string(file)?, env)?)?;
            }
        }
        if present(&service["build"]) {
            let build = if service["build"].is_string() {
                json!({"context":service["build"]})
            } else {
                service["build"].clone()
            };
            let object = build
                .as_object()
                .ok_or_else(|| error("Invalid build configuration"))?;
            if object.keys().any(|k| {
                ![
                    "context",
                    "dockerfile",
                    "dockerfile_inline",
                    "args",
                    "target",
                ]
                .contains(&k.as_str())
            }) {
                return Err(error("Unsafe build configuration"));
            }
            let context = within(
                root,
                &expand(build["context"].as_str().unwrap_or("."), env)?,
            )?;
            within(
                &context,
                &expand(build["dockerfile"].as_str().unwrap_or("Dockerfile"), env)?,
            )?;
        }
        let mut volumes = Vec::new();
        if present(&service["volumes"]) {
            for item in service["volumes"]
                .as_array()
                .ok_or_else(|| error("Invalid volume list"))?
            {
                let mut item = if let Some(s) = item.as_str() {
                    let parts = s.split(':').collect::<Vec<_>>();
                    if parts.len() > 3 {
                        return Err(error("Invalid mount syntax"));
                    }
                    let source = if parts.len() > 1 {
                        Some(parts[0])
                    } else {
                        None
                    };
                    json!({"type":if source.is_some_and(|s|s.starts_with('.')||s.starts_with('/')||s.contains('$')){"bind"}else{"volume"},"source":source,"target":if source.is_some(){parts[1]}else{parts[0]},"read_only":parts.get(2).is_some_and(|s|s.split(',').any(|v|v=="ro"))})
                } else {
                    item.clone()
                };
                if !item.is_object() {
                    return Err(error("Invalid mount configuration"));
                }
                let target = string(&item["target"])?;
                if !target.starts_with('/')
                    || target == "/"
                    || ["/proc", "/sys", "/dev"]
                        .iter()
                        .any(|p| target == *p || target.starts_with(&format!("{p}/")))
                {
                    return Err(error("Unsafe container mount target"));
                }
                if ["/var/run/docker.sock", "/run/docker.sock"].contains(&target) {
                    item = json!({"type":"bind","source":socket,"target":target,"read_only":true});
                } else if item["type"] == "bind" {
                    item["source"] = json!(within(root, &expand(string(&item["source"])?, env)?)?);
                } else if item["type"] != "volume" && item["type"] != "tmpfs" {
                    return Err(error("Unsupported volume driver"));
                }
                volumes.push(item);
            }
        }
        service["volumes"] = json!(volumes);
    }
    for section in ["configs", "secrets", "volumes", "networks"] {
        if !raw[section].is_null() {
            for value in raw[section]
                .as_object()
                .ok_or_else(|| error("Invalid resource section"))?
                .values()
            {
                if present(&value["external"]) {
                    return Err(error("External resources are not allowed"));
                }
                if ["configs", "secrets"].contains(&section) {
                    if present(&value["file"]) {
                        within(root, &expand(string(&value["file"])?, env)?)?;
                    }
                } else {
                    if present(&value["driver_opts"]) || present(&value["ipam"]) {
                        return Err(error("Custom storage/network drivers are not allowed"));
                    }
                    if !value["driver"].is_null()
                        && value["driver"]
                            != if section == "volumes" {
                                "local"
                            } else {
                                "bridge"
                            }
                    {
                        return Err(error("Unsafe storage/network driver"));
                    }
                }
            }
        }
    }
    Ok(())
}
pub struct Limits<'a> {
    pub memory: u64,
    pub cpu: f64,
    pub host_port: u16,
    pub container_port: u16,
    pub extra_address: Option<&'a str>,
}
pub fn normalize(
    raw: &mut Value,
    namespace: &str,
    source: &Path,
    socket: &Path,
    gateway: &str,
    limits: Limits<'_>,
) -> Result<Vec<String>> {
    let Limits {
        memory,
        cpu,
        host_port,
        container_port,
        extra_address,
    } = limits;
    let mut persistent = Vec::new();
    if raw["volumes"].is_null() {
        raw["volumes"] = json!({});
    }
    let mut volumes = raw["volumes"]
        .as_object()
        .ok_or_else(|| error("Invalid normalized volumes"))?
        .clone();
    for (name, value) in &mut volumes {
        if value.is_null() {
            *value = json!({});
        }
        value["name"] = json!(format!("{namespace}-{name}"));
        if let Some(v) = value.as_object_mut() {
            v.remove("external");
        }
        persistent.push(format!("{namespace}-{name}"));
    }
    let services = raw["services"]
        .as_object_mut()
        .ok_or_else(|| error("Missing normalized services"))?;
    if !services.contains_key(gateway) {
        return Err(error("Gateway service was not found"));
    }
    let weights = services
        .keys()
        .map(|name| {
            (
                name.clone(),
                if ["db", "postgres", "database"].contains(&name.as_str()) {
                    8
                } else if ["analytics", "logflare"].contains(&name.as_str()) {
                    5
                } else if ["functions", "studio", gateway].contains(&name.as_str()) {
                    3
                } else if ["realtime", "supavisor", "storage"].contains(&name.as_str()) {
                    2
                } else {
                    1
                },
            )
        })
        .collect::<BTreeMap<_, u64>>();
    let total = weights.values().sum::<u64>();
    for (name, service) in services.iter_mut() {
        service["container_name"] = json!(format!("{namespace}-{name}"));
        service["ports"] = json!([]);
        service["stop_grace_period"] = json!("20s");
        if service["environment"].is_null() {
            service["environment"] = json!({});
        }
        let image = service["image"].as_str().unwrap_or("").to_owned();
        if image.starts_with("nginx:") || image.contains("/nginx:") {
            service["environment"]
                .as_object_mut()
                .ok_or_else(|| error("Invalid environment"))?
                .entry("NGINX_ENTRYPOINT_WORKER_PROCESSES_AUTOTUNE")
                .or_insert(json!("1"));
        }
        if [
            "supabase/realtime",
            "supabase/logflare",
            "supabase/supavisor",
        ]
        .iter()
        .any(|v| image.contains(v))
        {
            let flags = service["environment"]["ERL_FLAGS"].as_str().unwrap_or("");
            service["environment"]["ERL_FLAGS"] =
                json!(format!("{flags} +S 2:2 +SDcpu 1:1 +SDio 1").trim());
        }
        if present(&service["build"]) {
            service["image"] = json!(format!("{namespace}-{name}:build"));
        } else if ["cbgpu/", "cbgpu-", "cbcpu_", "jupiterhub_"]
            .iter()
            .any(|s| image.starts_with(s))
        {
            return Err(error("Infrastructure images are reserved"));
        }
        service["cpus"] =
            json!((cpu * weights[name] as f64 / total as f64 * 10000.).round() / 10000.);
        let limit = memory * weights[name] / total;
        service["mem_limit"] = json!(limit);
        service["memswap_limit"] = json!(limit);
        let requested_pids = service["pids_limit"]
            .as_i64()
            .or_else(|| service["deploy"]["resources"]["limits"]["pids"].as_i64());
        service["pids_limit"] = json!(requested_pids.unwrap_or(-1));
        service["security_opt"] = json!(["no-new-privileges:true"]);
        service["oom_kill_disable"] = json!(false);
        service["logging"] =
            json!({"driver":"json-file","options":{"max-size":"10m","max-file":"2"}});
        let object = service
            .as_object_mut()
            .ok_or_else(|| error("Invalid normalized service"))?;
        object.remove("deploy");
        object.remove("oom_score_adj");
        if let Some(mounts) = service.get_mut("volumes").and_then(Value::as_array_mut) {
            for volume in mounts {
                if volume["type"] == "bind" {
                    let target = string(&volume["target"])?.to_owned();
                    if ["/var/run/docker.sock", "/run/docker.sock"].contains(&target.as_str()) {
                        volume["source"] = json!(socket);
                        volume["read_only"] = json!(true);
                        continue;
                    }
                    let src = within(source, string(&volume["source"])?)?;
                    if [
                        "/var/lib/postgresql/data",
                        "/var/lib/storage",
                        "/data",
                        "/var/lib/mysql",
                        "/var/lib/redis",
                    ]
                    .contains(&target.as_str())
                        || src.to_string_lossy().ends_with("/volumes/db/data")
                        || src.to_string_lossy().ends_with("/volumes/storage")
                    {
                        let hash = format!("{:x}", Sha256::digest(format!("{name}:{target}")));
                        let key = format!("persist-{}", &hash[..12]);
                        *volume = json!({"type":"volume","source":key,"target":target});
                        volumes.insert(key.clone(), json!({"name":format!("{namespace}-{key}")}));
                        persistent.push(format!("{namespace}-{key}"));
                    } else {
                        if !src.exists() {
                            return Err(error("Missing configuration mount"));
                        }
                        volume["read_only"] = json!(true);
                    }
                }
            }
        }
    }
    let mut addresses = vec!["127.0.0.1"];
    if let Some(address) = extra_address {
        addresses.push(address);
    }
    services[gateway]["ports"]=json!(addresses.into_iter().map(|address|json!({"target":container_port,"published":host_port.to_string(),"host_ip":address,"protocol":"tcp","mode":"ingress"})).collect::<Vec<_>>());
    raw["volumes"] = json!(volumes);
    if let Some(networks) = raw.get_mut("networks").and_then(Value::as_object_mut) {
        for (name, value) in networks {
            value["name"] = json!(format!("{namespace}-{name}"));
        }
    }
    raw["name"] = json!(namespace);
    persistent.sort();
    persistent.dedup();
    Ok(persistent)
}
