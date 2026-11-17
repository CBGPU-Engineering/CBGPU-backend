use std::process::Command;
#[test]
fn help_and_invalid_commands_are_resolved_before_operator_home_or_lock() {
    let binary = env!("CARGO_BIN_EXE_cbgpu-cutover");
    let help = Command::new(binary)
        .arg("--help")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8_lossy(&help.stdout).contains("Review/help never create operator state")
    );
    for arguments in [
        vec!["review", "--apply", "/tmp"],
        vec!["not-a-command"],
        vec!["rollback-deploy", "unexpected"],
    ] {
        let result = Command::new(binary)
            .args(arguments)
            .env_remove("HOME")
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!String::from_utf8_lossy(&result.stderr).contains("HOME missing"));
    }
}
