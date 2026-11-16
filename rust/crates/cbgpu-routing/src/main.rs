use cbgpu_routing::{Cloudflare, Result, Site, apply, credentials, plan};
use std::{env, path::PathBuf};
fn run() -> Result<()> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] {
        println!(
            "cbgpu-routing careers|developers|manufacturing|equipment-portals|deployments|latex|workspaces|project-namespaces [--apply] [--token-file PATH] [--cookie-db PATH] [--root PATH]"
        );
        return Ok(());
    }
    let site = Site::parse(&args[0])?;
    let mut apply_now = false;
    let mut token = None;
    let mut cookies = PathBuf::from(
        "/home/cbgpu/snap/firefox/common/.mozilla/firefox/5snylll4.default/cookies.sqlite",
    );
    let mut root = env::var_os("CBGPU_ROUTING_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/cbgpu/CBGPU-live/backend"));
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--apply" => apply_now = true,
            "--token-file" | "--cookie-db" | "--root" => {
                let flag = args[index].clone();
                index += 1;
                let value = PathBuf::from(args.get(index).ok_or("Missing flag value")?);
                match flag.as_str() {
                    "--token-file" => token = Some(value),
                    "--cookie-db" => cookies = value,
                    _ => root = value,
                }
            }
            _ => return Err("Unknown routing option".into()),
        }
        index += 1;
    }
    let fixed = PathBuf::from("/home/cbgpu/.config/cbgpu/cloudflare-routing-token");
    let override_path = env::var_os("CBGPU_CLOUDFLARE_ROUTING_TOKEN_FILE").map(PathBuf::from);
    let credential = credentials(
        site,
        token.as_deref(),
        override_path.as_deref(),
        &fixed,
        &cookies,
    )?;
    let mut api = Cloudflare::new(credential)?;
    let desired = plan(&mut api, site)?;
    println!("{}", desired.summary(apply_now));
    if apply_now {
        let backup = apply(&mut api, &desired, &root)?;
        println!(
            "Routing and DNS verified; private snapshot: {}",
            backup.display()
        )
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1)
    }
}
