use cbgpu_gpu_enrollment::{Config, router};
fn secret(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = std::env::var(name)?;
    let meta = std::fs::symlink_metadata(&path)?;
    if !meta.is_file() {
        return Err("Credential must be a regular private file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err("Credential file must not be group/world readable".into());
        }
    }
    let value = std::fs::read_to_string(path)?.trim().to_owned();
    if value.is_empty() || value.len() > 8192 {
        return Err("Invalid credential file".into());
    }
    Ok(value)
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listen: std::net::SocketAddr = std::env::var("CBGPU_GPU_REGISTRY_LISTEN")
        .unwrap_or("127.0.0.1:3548".into())
        .parse()?;
    if !listen.ip().is_loopback() {
        return Err("GPU registry must bind private loopback".into());
    }
    let config = Config {
        notebooks: if flag(std::env::var("CBGPU_GPU_NOTEBOOKS_ENABLED").ok().as_deref())? {
            let value = secret("CBGPU_GPU_NOTEBOOK_SIGNING_KEY_FILE")?;
            Some(std::sync::Arc::new(ed25519_dalek::SigningKey::from_bytes(
                &cbgpu_gpu_enrollment::unhex::<32>(&value).ok_or("Invalid notebook signing key")?,
            )))
        } else {
            None
        },
        diagnostics: match std::env::var("CBGPU_GPU_DIAGNOSTICS_ENABLED")
            .ok()
            .as_deref()
        {
            None | Some("" | "0" | "false") => None,
            Some("1" | "true") => {
                let value = secret("CBGPU_GPU_COMMAND_SIGNING_KEY_FILE")?;
                let bytes = cbgpu_gpu_enrollment::unhex::<32>(&value)
                    .ok_or("Command signing key must be 32-byte private hex")?;
                Some(std::sync::Arc::new(ed25519_dalek::SigningKey::from_bytes(
                    &bytes,
                )))
            }
            _ => return Err("Invalid diagnostics flag".into()),
        },
        enabled: flag(std::env::var("CBGPU_GPU_REGISTRY_ENABLED").ok().as_deref())?,
        backend_url: std::env::var("CBGPU_GPU_REGISTRY_BACKEND_URL")
            .unwrap_or("http://127.0.0.1:54321".into())
            .trim_end_matches('/')
            .into(),
        anon_key: secret("CBGPU_GPU_REGISTRY_ANON_KEY_FILE")?,
        service_key: secret("CBGPU_GPU_REGISTRY_SERVICE_KEY_FILE")?,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()?,
    };
    axum::serve(tokio::net::TcpListener::bind(listen).await?, router(config))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn flag(value: Option<&str>) -> Result<bool, Box<dyn std::error::Error>> {
    match value {
        None | Some("" | "0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        _ => Err("Invalid GPU registry flag".into()),
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn registry_flag_literals_fail_closed() {
        for x in [None, Some(""), Some("0"), Some("false")] {
            assert!(!super::flag(x).unwrap())
        }
        for x in ["1", "true"] {
            assert!(super::flag(Some(x)).unwrap())
        }
        for x in ["True", "yes", " 1", "false "] {
            assert!(super::flag(Some(x)).is_err())
        }
    }
}
