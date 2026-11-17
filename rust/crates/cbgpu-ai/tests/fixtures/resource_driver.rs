//! Native subprocess fixture: never touches Docker, NVIDIA or a real inference runtime.
use std::{io::Write, path::PathBuf};
const JOB: &str = "10000000-0000-4000-8000-000000000001";
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let exe = PathBuf::from(&args[0]);
    let parent = exe.parent().unwrap();
    let name = exe.file_name().unwrap().to_str().unwrap();
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(parent.join("calls"))
        .unwrap();
    writeln!(log, "{name} {}", args[1..].join(" ")).unwrap();
    match name {
        "nvidia-smi" => {
            if args[1].starts_with("--query-gpu") {
                println!("0, GPU-one, \"Test, Card\", 0, 0");
            }
        }
        "docker" => match args.get(1).map(String::as_str) {
            Some("exec") => {
                assert!(args.last().unwrap().starts_with("SELECT "));
                println!("[]");
            }
            Some("ps") => println!("cbgpu-ai-{JOB}\nnotebook-user\ncbgpu-ai-invalid"),
            Some("inspect") => {
                if parent.join("wrong-label").exists() {
                    println!("true other-job");
                } else {
                    println!("true {JOB}");
                }
            }
            Some("run") => {
                for expected in [
                    "--network",
                    "none",
                    "--cap-drop",
                    "ALL",
                    "--read-only",
                    "--pids-limit",
                    "--gpus",
                    "--security-opt",
                    "no-new-privileges",
                ] {
                    assert!(args.iter().any(|s| s == expected));
                }
            }
            Some("top") => println!("PID\n4242"),
            Some("rm") | Some("stop") => {}
            _ => panic!("Unexpected Docker command"),
        },
        "prlimit" => {
            let index = args.iter().position(|s| s == "--").unwrap();
            let status = std::process::Command::new(&args[index + 1])
                .args(&args[index + 2..])
                .status()
                .unwrap();
            std::process::exit(status.code().unwrap_or(1));
        }
        "native-runner" => {
            if exe.parent().unwrap().join("expect-scoped").exists() {
                assert_eq!(std::env::var("CBGPU_ACCOUNT_V2").unwrap(),"1");
                assert!(std::env::var("SERVICE_ROLE_KEY").is_err());
                assert!(std::env::var("CBGPU_AI_BACKEND_KEY_FILE").is_err());
            }
            assert_eq!(std::env::var("CUDA_VISIBLE_DEVICES").unwrap(), "GPU-one");
            let libraries = std::env::var("CBGPU_AI_NATIVE_LIBRARY_PATH").unwrap();
            assert_eq!(std::env::var("LD_LIBRARY_PATH").unwrap(), libraries);
            assert!(std::path::Path::new(&libraries).is_absolute());
            let folder = PathBuf::from(&args[2]);
            std::fs::write(
                folder.join("result.json"),
                r#"{"choices":[{"message":{"content":"Native fixture result"}}]}"#,
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        _ => panic!("Unexpected fixture executable"),
    }
}
