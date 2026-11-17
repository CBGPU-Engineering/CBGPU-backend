//! Offline extraction. Inputs derive only from a freshly authorized source and
//! operator-configured roots, never URLs/paths in a customer's search payload.
use crate::{Result, Source};
use anyhow::ensure;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[derive(Clone)]
enum PdfTool {
    Local(PathBuf),
    Sandbox(String),
}
#[derive(Clone)]
pub struct SourceFiles {
    materials_root: PathBuf,
    paper_origin: String,
    pdftotext: PdfTool,
}
impl SourceFiles {
    pub fn new(materials_root: PathBuf, paper_origin: &str, pdftotext: PathBuf) -> Result<Self> {
        let origin = reqwest::Url::parse(paper_origin)?;
        ensure!(
            origin.scheme() == "http"
                && matches!(
                    origin.host_str(),
                    Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
                )
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.query().is_none()
                && origin.fragment().is_none()
                && origin.path() == "/",
            "Paper source must be a fixed loopback origin"
        );
        ensure!(
            pdftotext.is_absolute(),
            "Configure an absolute native pdftotext executable"
        );
        ensure!(
            materials_root.is_absolute(),
            "Materials root must be absolute"
        );
        Ok(Self {
            materials_root,
            paper_origin: paper_origin.trim_end_matches('/').into(),
            pdftotext: PdfTool::Local(pdftotext),
        })
    }
    /// Digest is chosen by the trusted operator, never a request payload. The
    /// pinned OCR image supplies only native Poppler; no Python entrypoint runs.
    pub fn sandboxed(materials_root: PathBuf, paper_origin: &str, digest: &str) -> Result<Self> {
        ensure!(
            digest.starts_with("sha256:")
                && digest.len() == 71
                && digest[7..].bytes().all(|b| b.is_ascii_hexdigit()),
            "Use a reviewed immutable local OCR image digest"
        );
        let mut files = Self::new(
            materials_root,
            paper_origin,
            PathBuf::from("/usr/bin/pdftotext"),
        )?;
        files.pdftotext = PdfTool::Sandbox(digest.into());
        Ok(files)
    }
    pub(crate) fn extract(&self, source: &Source) -> Result<String> {
        ensure!(
            ["material", "paper"].contains(&source.kind.as_str()),
            "Invalid file source"
        );
        if source.kind == "material" {
            let hex = source.id.simple().to_string();
            let root = self.materials_root.canonicalize()?;
            let file = open_material(&root, &hex)?;
            let expected = source
                .size
                .ok_or_else(|| anyhow::anyhow!("Material size unavailable"))?;
            ensure!(
                expected <= 200 * 1024 * 1024 && file.metadata()?.len() == expected,
                "Material size changed or unsupported"
            );
            return extract_file(file, &source.name, &self.pdftotext);
        }
        let mut file = tempfile::tempfile()?;
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut response = client
            .get(format!(
                "{}/api/arxiv/papers/{}/pdf",
                self.paper_origin, source.id
            ))
            .header("Host", "arxiv.cbgpu.com")
            .send()?;
        ensure!(
            response.status().is_success(),
            "Published paper PDF unavailable"
        );
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut block = [0; 65536];
        loop {
            let n = response.read(&mut block)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            ensure!(total <= 21 * 1024 * 1024, "Paper PDF exceeds corpus limit");
            hasher.update(&block[..n]);
            file.write_all(&block[..n])?;
        }
        if let Some(expected) = source.size {
            ensure!(expected == total, "Paper size changed during download");
        }
        if let Some(expected) = &source.sha256 {
            ensure!(
                expected == &hex::encode(hasher.finalize()),
                "Paper digest changed during download"
            );
        }
        file.seek(SeekFrom::Start(0))?;
        extract_file(file, &source.name, &self.pdftotext)
    }
}
fn open_material(root: &PathBuf, hex: &str) -> Result<File> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags, openat};
        let directory = File::open(root)?;
        let shard = openat(
            &directory,
            &hex[..2],
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let fd = openat(
            &shard,
            hex,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let file = File::from(fd);
        ensure!(
            file.metadata()?.is_file(),
            "Material must be a regular file"
        );
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let shard = root.join(&hex[..2]);
        ensure!(
            fs::symlink_metadata(&shard)?.file_type().is_dir(),
            "Invalid material shard"
        );
        let path = shard.join(hex);
        ensure!(
            fs::symlink_metadata(&path)?.file_type().is_file()
                && path.canonicalize()?.starts_with(root),
            "Invalid material file"
        );
        Ok(File::open(path)?)
    }
}
fn extract_file(mut source: File, name: &str, pdftotext: &PdfTool) -> Result<String> {
    let suffix = name.rsplit('.').next().unwrap_or_default().to_lowercase();
    if ["txt", "md", "csv", "tex", "py", "log"].contains(&suffix.as_str()) {
        let mut bytes = vec![];
        source.take(800004).read_to_end(&mut bytes)?;
        return Ok(String::from_utf8_lossy(&bytes)
            .chars()
            .take(200000)
            .collect());
    }
    ensure!(suffix == "pdf", "Unsupported document format");
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("input.pdf");
    let output = dir.path().join("text.txt");
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&input)?;
    std::io::copy(&mut source, &mut destination)?;
    destination.sync_all()?;
    let output_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    let name = format!("cbgpu-corpus-{}", uuid::Uuid::new_v4());
    let (mut command, sandbox) = match pdftotext {
        PdfTool::Local(tool) => {
            ensure!(tool.is_file(), "Native pdftotext unavailable");
            let mut command = if cfg!(target_os = "linux") {
                let mut c = Command::new("/usr/bin/prlimit");
                c.args(["--as=17179869184", "--cpu=25", "--fsize=16777216", "--"])
                    .arg(tool);
                c
            } else {
                Command::new(tool)
            };
            command
                .args(["-f", "1", "-l", "100", "-enc", "UTF-8", "-nopgbrk"])
                .arg(&input)
                .arg("-");
            (command, false)
        }
        PdfTool::Sandbox(image) => {
            let mut command = Command::new("docker");
            command.args([
                "run",
                "--pull=never",
                "--rm",
                "--name",
                &name,
                "--network",
                "none",
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges",
                "--read-only",
                "--pids-limit",
                "64",
                "--cpus",
                "2",
                "--memory",
                "1g",
                "--tmpfs",
                "/tmp:rw,nosuid,nodev,size=128m",
                "--user",
                "65534:65534",
                "--mount",
                &format!("type=bind,src={},dst=/input.pdf,readonly", input.display()),
                "--label",
                "cbgpu.corpus.sandbox=true",
                "--entrypoint",
                "/usr/bin/timeout",
                image,
                "--signal=KILL",
                "25",
                "/usr/bin/pdftotext",
                "-f",
                "1",
                "-l",
                "100",
                "-enc",
                "UTF-8",
                "-nopgbrk",
                "/input.pdf",
                "-",
            ]);
            (command, true)
        }
    };
    let mut child = command
        .env_clear()
        .env("LANG", "C.UTF-8")
        .env("PATH", "/usr/bin:/bin")
        .env("OMP_NUM_THREADS", "4")
        .stdin(Stdio::null())
        .stdout(Stdio::from(output_file))
        .stderr(Stdio::null())
        .spawn()?;
    struct Cleanup(Option<String>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(name) = &self.0 {
                let _ = Command::new("docker")
                    .args(["rm", "-f", name])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
    let _cleanup = Cleanup(sandbox.then_some(name));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "PDF extraction failed or document encrypted"
            );
            break;
        }
        if Instant::now() >= deadline || fs::metadata(&output).is_ok_and(|m| m.len() > 16_777_216) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("PDF indexing deadline exceeded");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        fs::symlink_metadata(&output)?.file_type().is_file(),
        "Invalid PDF output"
    );
    let mut bytes = vec![];
    File::open(output)?.take(800004).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes)
        .chars()
        .take(200000)
        .collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn material_components_reject_symlinks_even_inside_root() {
        let root = tempfile::tempdir().unwrap();
        let hex = "10000000000040008000000000000001";
        fs::create_dir(root.path().join("10")).unwrap();
        fs::write(root.path().join("10").join(hex), b"synthetic").unwrap();
        assert!(open_material(&root.path().to_path_buf(), hex).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            fs::rename(root.path().join("10"), root.path().join("actual")).unwrap();
            symlink(root.path().join("actual"), root.path().join("10")).unwrap();
            assert!(open_material(&root.path().to_path_buf(), hex).is_err());
            fs::remove_file(root.path().join("10")).unwrap();
            fs::rename(root.path().join("actual"), root.path().join("10")).unwrap();
            fs::remove_file(root.path().join("10").join(hex)).unwrap();
            fs::write(root.path().join("target"), b"other").unwrap();
            symlink(root.path().join("target"), root.path().join("10").join(hex)).unwrap();
            assert!(open_material(&root.path().to_path_buf(), hex).is_err());
        }
    }
    fn pdf() -> Vec<u8> {
        let objects=["<< /Type /Catalog /Pages 2 0 R >>".to_string(),"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".into(),{let text="BT /F1 12 Tf 40 300 Td (Synthetic corpus needle document) Tj ET";format!("<< /Length {} >>\nstream\n{}\nendstream",text.len(),text)},"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()];
        let mut output = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0];
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(output.len());
            write!(output, "{} 0 obj\n{}\nendobj\n", i + 1, obj).unwrap();
        }
        let start = output.len();
        write!(output, "xref\n0 {}\n0000000000 65535 f \n", offsets.len()).unwrap();
        for offset in offsets.iter().skip(1) {
            writeln!(output, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            output,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            offsets.len(),
            start
        )
        .unwrap();
        output
    }
    #[test]
    #[ignore = "Own offline disposable Poppler container, requires explicit reviewed CBGPU_CORPUS_TEST_OCR_IMAGE digest"]
    fn native_sandbox_pdf_extracts_synthetic_only() {
        let image =
            std::env::var("CBGPU_CORPUS_TEST_OCR_IMAGE").expect("Explicit local OCR image digest");
        let bytes = pdf();
        let expected = hex::encode(Sha256::digest(&bytes));
        let size = bytes.len() as u64;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = vec![];
            let mut one = [0; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut one).unwrap();
                request.push(one[0]);
            }
            let request = String::from_utf8(request).unwrap().to_lowercase();
            assert!(request.contains("host: arxiv.cbgpu.com"));
            assert!(!request.contains("authorization:"));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let files = SourceFiles::sandboxed(root.path().into(), &origin, &image).unwrap();
        let source = Source {
            kind: "paper".into(),
            id: uuid::Uuid::new_v4(),
            version: "1".into(),
            name: "synthetic.pdf".into(),
            label: "Synthetic".into(),
            href: "/synthetic".into(),
            text: String::new(),
            size: Some(size),
            sha256: Some(expected),
        };
        let text = files.extract(&source).unwrap();
        server.join().unwrap();
        assert!(text.contains("Synthetic corpus needle document"));
    }
    #[test]
    fn source_configuration_rejects_remote_or_mutable_image() {
        let root = tempfile::tempdir().unwrap();
        for url in [
            "https://example.com/",
            "http://u:secret@127.0.0.1/",
            "http://127.0.0.1/?url=x",
        ] {
            assert!(
                SourceFiles::new(root.path().into(), url, PathBuf::from("/usr/bin/pdftotext"))
                    .is_err()
            );
        }
        assert!(
            SourceFiles::sandboxed(
                root.path().into(),
                "http://127.0.0.1:3000",
                "cbgpu-ocr:latest"
            )
            .is_err()
        );
    }
}
#[cfg(test)]
mod transport_tests {
    use super::*;
    #[test]
    fn published_pdf_redirects_and_digest_mismatch_fail_closed() {
        for redirect in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                let mut input = [0; 4096];
                let _ = s.read(&mut input);
                if redirect {
                    s.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://example.com/private\r\nContent-Length: 0\r\n\r\n").unwrap();
                } else {
                    s.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nPDF",
                    )
                    .unwrap();
                }
            });
            let root = tempfile::tempdir().unwrap();
            let files = SourceFiles::new(
                root.path().into(),
                &origin,
                PathBuf::from("/usr/bin/pdftotext"),
            )
            .unwrap();
            let source = Source {
                kind: "paper".into(),
                id: uuid::Uuid::new_v4(),
                version: "1".into(),
                name: "synthetic.pdf".into(),
                label: "Synthetic".into(),
                href: "/synthetic".into(),
                text: String::new(),
                size: Some(3),
                sha256: Some("0".repeat(64)),
            };
            assert!(files.extract(&source).is_err());
            server.join().unwrap();
        }
    }
}
