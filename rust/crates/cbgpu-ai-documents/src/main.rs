use std::{io::Write, path::Path};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--version"] {
        println!("cbgpu-ai-documents {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let result = if args.len() != 2 {
        Err("Usage: cbgpu-ai-documents FILE SUFFIX".into())
    } else if [".pdf", ".png", ".jpg", ".jpeg", ".webp", ".tif", ".tiff"]
        .contains(&args[1].as_str())
    {
        cbgpu_ai_documents::extract_visual(Path::new(&args[0]), &args[1])
    } else {
        cbgpu_ai_documents::extract_text(Path::new(&args[0]), &args[1])
    };
    let (value, failed) = match result {
        Ok(r) => (serde_json::json!({"result":r}), false),
        Err(e) => (
            serde_json::json!({"error":e.chars().take(300).collect::<String>()}),
            true,
        ),
    };
    let mut out = std::io::stdout().lock();
    if serde_json::to_writer(&mut out, &value).is_err() || writeln!(out).is_err() {
        std::process::exit(1);
    }
    if failed {
        std::process::exit(1);
    }
}
