//! Explicitly opted-in, non-networked native OCR contract. No real uploads or user data.
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};
fn image() -> String {
    std::env::var("CBGPU_OCR_TEST_IMAGE")
        .expect("Set CBGPU_OCR_TEST_IMAGE to a built native worker image")
}
fn run(path: &Path, suffix: &str) -> (bool, Vec<Value>) {
    let out = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--memory",
            "512m",
            "--cpus",
            "2",
            "--pids-limit",
            "64",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,size=128m",
            "--user",
            "1000:1000",
            "--mount",
            &format!("type=bind,src={},dst=/input,readonly", path.display()),
            &image(),
            "/input",
            suffix,
        ])
        .output()
        .unwrap();
    let events = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).expect("worker emits NDJSON only"))
        .collect();
    (out.status.success(), events)
}
fn pdf() -> Vec<u8> {
    let sentence = "CBGPU native Rust document extraction preserves readable research evidence.";
    let stream = format!(
        "BT /F1 12 Tf 16 TL 30 760 Td {} ET",
        format!("({sentence}) Tj T*\n").repeat(6)
    );
    let objects=["<< /Type /Catalog /Pages 2 0 R >>".to_owned(),"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".into(),"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),format!("<< /Length {} >>\nstream\n{stream}\nendstream",stream.len())];
    let mut body = "%PDF-1.4\n".to_owned();
    let mut offsets = vec![];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(body.len());
        body.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
    }
    let position = body.len();
    body.push_str("xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        body.push_str(&format!("{offset:010} 00000 n \n"));
    }
    body.push_str(&format!(
        "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{position}\n%%EOF"
    ));
    body.into_bytes()
}
fn fixture(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
}
fn raster(format: &str, duplicates: &str) -> Vec<u8> {
    let output = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--network",
            "none",
            "--read-only",
            "--memory",
            "384m",
            "--cpus",
            "2",
            "--pids-limit",
            "64",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,size=128m",
            "--entrypoint",
            "convert",
            &image(),
            "-background",
            "white",
            "-fill",
            "black",
            "-font",
            "DejaVu-Sans",
            "-pointsize",
            "56",
            "label:CBGPU Rust OCR",
            "-duplicate",
            duplicates,
            "-compress",
            "Zip",
            format,
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "synthetic image generation failed");
    output.stdout
}
#[test]
#[ignore = "requires explicit CBGPU_OCR_TEST_IMAGE and Docker; synthetic private files only"]
fn offline_pdf_png_multiframe_and_rejection_contract() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("input");
    fixture(&file, &pdf());
    let (ok, events) = run(&file, ".pdf");
    assert!(ok, "{events:?}");
    let result = &events.last().unwrap()["result"];
    assert_eq!(result["pages"], 1);
    assert_eq!(result["ocr_page_numbers"], serde_json::json!([]));
    assert!(result["chunks"][0][1].as_str().unwrap().contains("CBGPU"));
    assert!(events.len() >= 3);
    for (format, duplicates, suffix, pages) in
        [("png:-", "0", ".png", 1), ("tiff:-", "1", ".tiff", 2)]
    {
        fixture(&file, &raster(format, duplicates));
        let (ok, events) = run(&file, suffix);
        assert!(ok, "{events:?}");
        let result = &events.last().unwrap()["result"];
        assert_eq!(result["pages"], pages);
        assert_eq!(
            result["ocr_page_numbers"].as_array().unwrap().len(),
            pages as usize
        );
        assert!(
            result["chunks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|chunk| chunk[1].as_str().unwrap().contains("CBGPU"))
        );
    }
    fixture(&file, &raster("tiff:-", "20"));
    let (ok, events) = run(&file, ".tiff");
    assert!(!ok);
    assert!(
        events.last().unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("20")
    );
    fixture(&file, b"not a valid image");
    let (ok, events) = run(&file, ".png");
    assert!(!ok);
    assert!(events.last().unwrap()["error"].is_string());
}
