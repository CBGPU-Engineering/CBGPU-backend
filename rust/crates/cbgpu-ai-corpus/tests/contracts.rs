use cbgpu_ai_corpus::{SourceKey, chunk_text, validate_vector};
#[test]
fn chunking_preserves_unicode_overlap_and_bounds() {
    let text = "한".repeat(200001);
    let chunks = chunk_text(&text);
    assert_eq!(chunks.len(), 400);
    assert_eq!(chunks[0].chars().count(), 650);
    assert_eq!(chunks.last().unwrap().chars().count(), 500);
    assert!(chunk_text(" \n\t").is_empty());
}
#[test]
fn source_reference_and_vector_bounds() {
    assert!(SourceKey::parse("material:00000000-0000-4000-8000-000000000001").is_ok());
    for value in [
        "doc:school",
        "file:../../secret",
        "note:invalid",
        "paper:00000000-0000-4000-8000-00000000000A",
    ] {
        assert!(SourceKey::parse(value).is_err());
    }
    let mut v = vec![0.; 384];
    v[0] = 1.;
    assert!(validate_vector(&v).is_ok());
    v[2] = f32::NAN;
    assert!(validate_vector(&v).is_err());
    assert!(validate_vector(&vec![0.; 384]).is_err());
}
#[cfg(feature = "e5")]
#[test]
#[ignore = "Explicit local public E5 assets only; CPU smoke, no GPU or user corpus"]
fn actual_native_e5_cpu_smoke() {
    use cbgpu_ai_corpus::{Embedder, NativeE5};
    let root = std::env::var_os("CBGPU_CORPUS_TEST_E5").expect("Explicit model directory");
    let model = NativeE5::load(std::path::Path::new(&root)).unwrap();
    let v = model
        .embed(&["Synthetic school research document".into()], false)
        .unwrap();
    assert_eq!(v.len(), 1);
    validate_vector(&v[0]).unwrap();
}
