//! Same-origin user uploads must never become executable authenticated documents.
#[test]
fn storage_responses_sandbox_documents_without_changing_api_routes() {
    let config = include_str!("../../../../volumes/api/envoy/lds.template.yaml");
    let storage = config.split_once("prefix: /storage/v1/").unwrap().1;
    let storage = storage
        .split("                      - match:")
        .next()
        .unwrap();
    assert!(storage.contains("key: Content-Security-Policy"));
    assert!(storage.contains(
        "sandbox; default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    ));
    assert!(storage.contains("append_action: OVERWRITE_IF_EXISTS_OR_ADD"));
    assert!(storage.contains("key: X-Content-Type-Options"));
    assert!(storage.contains("cluster: storage"));
    assert!(storage.contains("prefix_rewrite: /"));
    assert_eq!(
        config.matches("key: Content-Security-Policy").count(),
        1,
        "Document sandbox must not replace Auth/REST policies"
    );
}
