use crate::{Result, fail};
use serde_json::{Value, json};
fn rx(pattern: &str, value: &str) -> bool {
    regex::Regex::new(pattern)
        .expect("static config pattern")
        .is_match(value)
}
pub fn slug(value: &str) -> Result<String> {
    let s = value.to_ascii_lowercase();
    let reserved = "www app api admin developers developer researcher student docs arxiv gpu mail send rsend resend autoconfig autodiscover status cbgpu test localhost backend auth files assets dashboard";
    if !rx(r"^[a-z][a-z0-9-]{2,34}$", &s)
        || s.ends_with('-')
        || s.starts_with("server-")
        || rx(r"-[a-f0-9]{8}$", &s)
        || reserved.split_whitespace().any(|v| v == s)
    {
        return Err(fail(
            "Use a unique project name:3–35 lowercase letters, numbers and hyphens",
            400,
        ));
    }
    Ok(s)
}
fn field(input: &Value, previous: &Value, key: &str, default: &str) -> Result<String> {
    match input.get(key).or_else(|| previous.get(key)) {
        None | Some(Value::Null) => Ok(default.into()),
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(fail("Invalid project configuration field", 400)),
    }
}
pub fn validate(input: &Value, previous: &Value, old_secrets: &Value) -> Result<(Value, Value)> {
    let name = slug(&field(input, previous, "slug", "")?)?;
    let repository = field(input, previous, "repository", "")?;
    let url =
        url::Url::parse(&repository).map_err(|_| fail("Enter a GitHub repository URL", 400))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !rx(
            r"^/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?/?$",
            url.path(),
        )
    {
        return Err(fail(
            "Use https://github.com/owner/repository without credentials",
            400,
        ));
    }
    let branch = field(input, previous, "branch", "main")?;
    if !rx(r"^[A-Za-z0-9][A-Za-z0-9_./-]{0,99}$", &branch) || branch.contains("..") {
        return Err(fail("Invalid branch", 400));
    }
    let framework = field(input, previous, "framework", "auto")?;
    if !["auto", "next", "vite", "astro", "static", "other"].contains(&framework.as_str()) {
        return Err(fail("Unsupported framework", 400));
    }
    let auto = input
        .get("auto_deploy")
        .or_else(|| previous.get("auto_deploy"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut config = json!({"slug":name,"repository":url.as_str().trim_end_matches('/'),"branch":branch,"framework":framework,"auto_deploy":auto});
    for name in [
        "root_directory",
        "output_directory",
        "install_command",
        "build_command",
    ] {
        let value = field(input, previous, name, "")?;
        if value.len() > 500 || value.contains(['\0', '\r', '\n']) {
            return Err(fail("Invalid build configuration", 400));
        }
        if name.ends_with("directory")
            && (value.starts_with(['/', '\\']) || value.split(['/', '\\']).any(|part| part == ".."))
        {
            return Err(fail("Directories must stay inside the repository", 400));
        }
        config[name] = json!(value);
    }
    let mut secrets = json!({"git_token":old_secrets["git_token"].as_str().unwrap_or(""),"env":old_secrets["env"].as_object().cloned().unwrap_or_default()});
    if let Some(value) = input.get("git_token") {
        let value = value
            .as_str()
            .filter(|v| v.len() <= 1000 && !v.contains(['\r', '\n']))
            .ok_or_else(|| fail("Invalid Git token", 400))?;
        secrets["git_token"] = json!(value);
    }
    if let Some(value) = input.get("env") {
        let values = value
            .as_object()
            .ok_or_else(|| fail("Use an environment variable object", 400))?;
        for (key, value) in values {
            if !rx(r"^[A-Za-z_][A-Za-z0-9_]{0,99}$", key)
                || [
                    "NODE_OPTIONS",
                    "LD_PRELOAD",
                    "PATH",
                    "HOME",
                    "GIT_CONFIG",
                    "DOCKER_HOST",
                ]
                .iter()
                .any(|n| key == n || key.starts_with(&format!("{n}_")))
            {
                return Err(fail("Reserved or invalid environment variable", 400));
            }
            if value.is_null() {
                secrets["env"].as_object_mut().unwrap().remove(key);
            } else {
                let text = value
                    .as_str()
                    .filter(|v| v.len() <= 8192)
                    .ok_or_else(|| fail("Environment value too long", 400))?;
                secrets["env"][key] = json!(text);
            }
        }
    }
    if secrets["env"].as_object().unwrap().len() > 40 {
        return Err(fail("Use up to40 environment variables", 400));
    }
    Ok((config, secrets))
}
pub fn hostname(value: &str) -> Result<String> {
    let host = value.trim().to_ascii_lowercase();
    if host.len() > 200
        || host.parse::<std::net::IpAddr>().is_ok()
        || !host.contains('.')
        || host == "cbgpu.com"
        || host.ends_with(".cbgpu.com")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || !host
            .split('.')
            .all(|v| rx(r"^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$", v))
    {
        return Err(fail(
            "Use your own public domain without URL, path or port",
            400,
        ));
    }
    Ok(host)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roots_secrets_and_repository_are_validated_without_deleting_existing_values() {
        let input = json!({"slug":"research-web","repository":"https://github.com/org/repo","root_directory":"frontend/app","env":{"API":"token"}});
        let (config, secrets) = validate(&input, &json!({}), &json!({})).unwrap();
        let (_, updated) = validate(
            &json!({"env":{"NEXT":"value","API":null}}),
            &config,
            &secrets,
        )
        .unwrap();
        assert_eq!(updated["env"], json!({"NEXT":"value"}));
        for root in ["../secret", "/root", "x/../../root", "x\\..\\z"] {
            assert!(validate(&json!({"root_directory":root}), &config, &secrets).is_err())
        }
        assert!(
            validate(
                &json!({"repository":"https://user:token@github.com/a/b"}),
                &config,
                &secrets
            )
            .is_err()
        );
    }
    #[test]
    fn domains_and_reserved_names_fail_closed() {
        for h in [
            "cbgpu.com",
            "admin.cbgpu.com",
            "127.0.0.1",
            "*.example.com",
            "example.com/path",
            "a.local",
            "evil.com:443",
        ] {
            assert!(hostname(h).is_err())
        }
        assert_eq!(hostname("Docs.Example.com").unwrap(), "docs.example.com");
        assert!(slug("admin").is_err());
    }
}

/// Explicitly disabled or absent is legacy; malformed enabled configuration fails closed.
pub fn resource_enforced(account: Option<&str>, schools: Option<&str>) -> bool {
    [account, schools].into_iter().flatten().any(|value| {
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false"
        )
    })
}
#[cfg(test)]
mod resource_flag_tests {
    use super::*;
    #[test]
    fn schools_and_malformed_flags_cannot_disable_scope_checks() {
        for (account, schools) in [
            (Some("true"), None),
            (Some("1"), None),
            (None, Some("1")),
            (Some("false"), Some("true")),
            (Some("typo"), None),
        ] {
            assert!(resource_enforced(account, schools))
        }
        for (account, schools) in [
            (None, None),
            (Some("false"), None),
            (Some("0"), Some("false")),
        ] {
            assert!(!resource_enforced(account, schools))
        }
    }
}
