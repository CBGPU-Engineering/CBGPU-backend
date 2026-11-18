//! Separate private transport service; never executes a host command or controls legacy GPUs.
use std::{net::SocketAddr,path::Path,sync::Arc,time::Duration};
use cbgpu_notebook_relay::{Gateway,authority::PostgresAuthority};
fn secret(name:&str)->Result<String,Box<dyn std::error::Error>>{
 use std::os::unix::fs::MetadataExt;
 let path=std::env::var(name)?;if !Path::new(&path).is_absolute(){return Err("Secret path must be absolute".into())}
 let meta=std::fs::symlink_metadata(&path)?;if !meta.is_file()||meta.mode()&0o077!=0||meta.len()>16384{return Err("Secret file must be private and regular".into())}
 let s=std::fs::read_to_string(path)?;Ok(s.trim().into())
}
#[tokio::main]async fn main()->Result<(),Box<dyn std::error::Error>>{
 let listen:SocketAddr=std::env::var("CBGPU_NOTEBOOK_RELAY_LISTEN").unwrap_or_else(|_|"127.0.0.1:3557".into()).parse()?;if !listen.ip().is_loopback(){return Err("Relay must bind private loopback".into())}
 let backend=std::env::var("CBGPU_GPU_BACKEND_URL").unwrap_or_else(|_|"http://127.0.0.1:54321".into());let endpoint=url::Url::parse(&backend)?;if !matches!(endpoint.host_str(),Some("127.0.0.1"|"localhost"))||endpoint.scheme()!="http"||endpoint.username()!=""||endpoint.password().is_some()||endpoint.path()!="/"||endpoint.query().is_some(){return Err("Invalid private authority endpoint".into())}
 let seed=secret("CBGPU_GPU_NOTEBOOK_SIGNING_KEY_FILE")?;let signer=ed25519_dalek::SigningKey::from_bytes(&cbgpu_notebook_protocol::unhex::<32>(&seed).ok_or("Invalid signing key")?);
 let authority=Arc::new(PostgresAuthority{backend,service_key:secret("CBGPU_GPU_SERVICE_ROLE_KEY_FILE")?,client:reqwest::Client::builder().timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build()?});
 let mut gateway=Gateway::new(authority,signer);let key=secret("CBGPU_NOTEBOOK_BOOTSTRAP_KEY_FILE")?;if key.len()!=64||!key.bytes().all(|v|v.is_ascii_hexdigit()){return Err("Invalid bootstrap key".into())}Arc::get_mut(&mut gateway).ok_or("Invalid gateway state")?.bootstrap_key=key;
 let listener=tokio::net::TcpListener::bind(listen).await?;eprintln!("{{\"service\":\"cbgpu-notebook-relay\",\"event\":\"ready\"}}");
 axum::serve(listener,cbgpu_notebook_relay::router(gateway)).with_graceful_shutdown(async{let _=tokio::signal::ctrl_c().await;}).await?;Ok(())
}
