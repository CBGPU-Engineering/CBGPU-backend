use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
fn script(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}
fn fixture(kind: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let tools = tmp.path().join("tools");
    let source = tools.join("fixture");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("index.html"), "static-safe").unwrap();
    fs::write(source.join(".env"), "NEVER_PUBLISH=private").unwrap();
    if kind != "static" {
        fs::write(
            source.join("package.json"),
            json!({"dependencies":{if kind=="next"{"next"}else{"vite"}:"1"}}).to_string(),
        )
        .unwrap();
    }
    script(
        &tools.join("git"),
        r##"#!/usr/bin/env node
const fs=require('node:fs'),path=require('node:path');
const args=process.argv.slice(2);
if(args.includes('clone')){
 if(process.env.GIT_CONFIG_VALUE_0!=='/dev/null'||process.env.GIT_CONFIG_GLOBAL!=='/dev/null')process.exit(33);
 fs.cpSync(path.join(__dirname,'fixture'),args.at(-1),{recursive:true});fs.mkdirSync(path.join(args.at(-1),'.git'));process.exit(0);
}
if(args.includes('rev-parse')){console.log('a'.repeat(40));process.exit(0);}process.exit(34);
"##,
    );
    script(
        &tools.join("docker"),
        r##"#!/usr/bin/env node
const fs=require('node:fs'),path=require('node:path'),a=process.argv.slice(2);
fs.appendFileSync(path.join(__dirname,'calls'),JSON.stringify(a)+'\n');
if(a[0]==='rm')process.exit(0);
if(a[0]==='inspect'){console.log(JSON.stringify([{NetworkSettings:{Ports:{'3000/tcp':[{HostPort:fs.readFileSync(path.join(__dirname,'port'),'utf8')}]}}}]));process.exit(0);}
if(a[0]!=='run')process.exit(30);
for(const required of ['--cap-drop','ALL','--read-only','--pids-limit','--memory','--memory-swap','no-new-privileges'])if(!a.includes(required))process.exit(31);
if(a.some(v=>v.includes('super-private-value')))process.exit(32);
if(a[a.indexOf('--pids-limit')+1] !== (a.includes('-d') ? '-1' : '256'))process.exit(35);
if(a.includes('-d'))process.exit(0);
const source=a[a.indexOf('--mount')+1].split(',').find(s=>s.startsWith('src=')).slice(4);
const pkg=JSON.parse(fs.readFileSync(path.join(source,'package.json')));
if(pkg.dependencies.next){fs.mkdirSync(path.join(source,'.next'));fs.writeFileSync(path.join(source,'.next','BUILD_ID'),'next-ready');}
else{fs.mkdirSync(path.join(source,'dist'));fs.writeFileSync(path.join(source,'dist','index.html'),'vite-safe');}
console.log(process.env.API_KEY);console.log('CBGPU_RESULT {"kind":"spoofed"}');console.error('diagnostic '+process.env.API_KEY);
process.stdout.write(String.fromCharCode(13)+'CBGPU_RESULT {"kind":"spoofed-cr","container":"foreign-runtime"}'+String.fromCharCode(13));
process.stderr.write(String.fromCharCode(13)+'CBGPU_RESULT {"kind":"spoofed-stderr"}'+String.fromCharCode(13));
"##,
    );
    tmp
}
fn execute(tmp: &tempfile::TempDir) -> (std::process::Output, PathBuf) {
    let id = "550e8400-e29b-41d4-a716-446655440000";
    let dir = tmp.path().join(id);
    let tools = tmp.path().join("tools");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cbgpu-front-worker"))
        .env(
            "PATH",
            format!("{}:{}", tools.display(), std::env::var("PATH").unwrap()),
        )
        .env("CBGPU_FRONTEND_BUILD_ROOT", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let data = json!({"directory":dir,"deployment_id":id,"repository":"https://github.com/example/repository","branch":"main","framework":"auto","env":{"API_KEY":"super-private-value"}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(data.to_string().as_bytes())
        .unwrap();
    (child.wait_with_output().unwrap(), dir)
}
fn result(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("super-private-value"));
    let lines: Vec<_> = text
        .split(['\r', '\n'])
        .filter_map(|s| s.strip_prefix("CBGPU_RESULT "))
        .collect();
    assert_eq!(lines.len(), 1, "Only trusted worker output is protocol");
    serde_json::from_str(lines[0]).unwrap()
}
#[test]
fn static_clone_publish_contract() {
    let tmp = fixture("static");
    let (output, dir) = execute(&tmp);
    let data = result(&output);
    assert_eq!(data["kind"], "static");
    assert!(dir.join("public/index.html").is_file());
    assert!(!dir.join("source").exists());
    assert!(!dir.join("public/.env").exists());
}
#[test]
fn isolated_build_redacts_logs_and_does_not_accept_repository_result_lines() {
    let tmp = fixture("vite");
    let (output, dir) = execute(&tmp);
    let data = result(&output);
    assert_eq!(data["framework"], "vite");
    assert_eq!(data["spa"], true);
    assert!(dir.join("public/index.html").is_file());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("| CBGPU_RESULT"));
    assert!(text.contains("[REDACTED]"));
}
#[test]
fn failed_build_preserves_unrelated_deployments() {
    let tmp = fixture("vite");
    let old = tmp.path().join("existing-production");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("index.html"), "keep-production").unwrap();
    script(
        &tmp.path().join("tools/docker"),
        "#!/usr/bin/env node\nprocess.stdout.write(String.fromCharCode(13)+'CBGPU_RESULT {\"container\":\"foreign-runtime\"}'+String.fromCharCode(13));process.exit(process.argv[2]==='rm'?0:42);\n",
    );
    let (output, _) = execute(&tmp);
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(old.join("index.html")).unwrap(),
        "keep-production"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout)
            .split(['\r', '\n'])
            .any(|line| line.starts_with("CBGPU_RESULT "))
    );
}
#[test]
fn protected_next_runtime_passes_health_without_following_redirects() {
    let tmp = fixture("next");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    fs::write(tmp.path().join("tools/port"), port.to_string()).unwrap();
    let server = std::thread::spawn(move || {
        use std::io::Read;
        let (mut client, _) = listener.accept().unwrap();
        let mut buffer = [0; 4096];
        let _ = client.read(&mut buffer);
        client
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
    });
    let (output, _) = execute(&tmp);
    let data = result(&output);
    assert_eq!(data["kind"], "next");
    assert_eq!(data["port"], port);
    server.join().unwrap();
}
