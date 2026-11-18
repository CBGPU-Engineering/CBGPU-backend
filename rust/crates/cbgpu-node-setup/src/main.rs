#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
use cbgpu_node_setup::{App, router};
use std::{sync::Arc, time::Duration};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--version") {
        println!("CBGPU Node Setup {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let app = Arc::new(App::new(port)?);
    let launcher = app
        .launcher_file()
        .map_err(|_| "Private browser launcher unavailable")?;
    let url = reqwest::Url::from_file_path(&launcher)
        .map_err(|_| "Private browser launcher unavailable")?;
    println!("CBGPU setup started. Its private browser session expires in 20 minutes.");
    if !std::env::args().any(|a| a == "--no-browser") {
        #[cfg(target_os = "linux")]
        let _ = std::process::Command::new("xdg-open")
            .arg(url.as_str())
            .spawn();
        #[cfg(target_os = "windows")]
        let _ =
            std::process::Command::new(cbgpu_node_setup::windows::system_program("rundll32.exe")?)
                .args(["url.dll,FileProtocolHandler", url.as_str()])
                .spawn();
    }
    // No tokens or private file paths are printed, including in failures.
    let result=axum::serve(listener,router(app)).with_graceful_shutdown(async{
        #[cfg(unix)] let terminate=async{if let Ok(mut signal)=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()){signal.recv().await;}};
        #[cfg(not(unix))] let terminate=std::future::pending::<()>();
        tokio::select!{_=tokio::signal::ctrl_c()=>{},_=terminate=>{},_=tokio::time::sleep(Duration::from_secs(1200))=>{}}
    }).await;
    if result.is_err() {
        eprintln!("Setup stopped because its local connection failed.");
    }
    Ok(())
}
