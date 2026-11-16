//! Native fake subprocesses for isolated operations tests. No real systemctl/Docker.
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::PathBuf,
};
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let exe = PathBuf::from(&args[0]);
    let name = exe.file_name().unwrap().to_str().unwrap();
    let root = exe.parent().unwrap();
    let log = root.join("calls");
    let mut output = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .unwrap();
    if name == "docker" {
        if args.get(1).map(String::as_str) == Some("inspect") {
            writeln!(output, "docker inspect").unwrap();
            println!("127.0.0.2");
            return;
        }
        let mut sql = String::new();
        std::io::stdin().read_to_string(&mut sql).unwrap();
        let action = if sql.contains("SELECT count(*) FROM pg_roles") {
            println!("0");
            "roles-check"
        } else if sql.contains("SELECT count(*) FROM pg_database") {
            println!("0");
            "database-check"
        } else if sql.contains("CREATE ROLE") {
            assert!(
                sql.contains("NOSUPERUSER")
                    && sql.contains("NOCREATEDB")
                    && sql.contains("NOINHERIT")
            );
            "create-roles"
        } else if sql.contains("CREATE DATABASE") {
            if root.join("fail-create-db").exists() {
                std::process::exit(1)
            }
            "create-db"
        } else if sql.contains("DROP DATABASE") {
            assert!(sql.contains("cbgpu_runtime_test"));
            "drop-test-db"
        } else if sql.contains("REVOKE cbgpu_runtime_owner") {
            "revoke-owner"
        } else if sql.contains("ALTER ROLE cbgpu_runtime_owner NOLOGIN") {
            "disable-owner"
        } else if sql.contains("SET ROLE cbgpu_runtime_owner") {
            "migrate-test"
        } else {
            "sql"
        };
        writeln!(output, "docker {action}").unwrap();
    } else if name == "systemctl" {
        writeln!(output, "systemctl {}", args[1..].join(" ")).unwrap();
        if root.join("fail-restart-once").exists() && args.iter().any(|a| a == "restart") {
            std::fs::remove_file(root.join("fail-restart-once")).unwrap();
            std::process::exit(1);
        }
    } else if name == "git" {
        println!("0123456789abcdef0123456789abcdef01234567");
    } else if name == "cbgpu-core" {
        writeln!(output, "core {}", args[1..].join(" ")).unwrap();
        if args.get(1).map(String::as_str) == Some("migrate") {
            assert!(
                std::env::var("CBGPU_RUNTIME_DATABASE_URL")
                    .unwrap()
                    .contains("cbgpu_runtime_owner")
            );
        } else {
            assert_eq!(args.get(2).map(String::as_str), Some("--dry-run"));
        }
    } else if name == "cargo" {
        writeln!(output, "cargo {}", args[1..].join(" ")).unwrap();
        assert!(
            std::env::var("CBGPU_TEST_DATABASE_URL")
                .unwrap()
                .ends_with("/cbgpu_runtime_test")
        );
        assert_eq!(std::env::var("CARGO_BUILD_JOBS").unwrap(), "4");
    } else {
        panic!("unknown fake subprocess")
    }
}
