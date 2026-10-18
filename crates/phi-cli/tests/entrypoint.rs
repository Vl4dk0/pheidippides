#[test]
fn skill_install_help_and_user_error_exit_codes() -> phi_core::Result<()> {
    use std::process::Command;
    let binary = env!("CARGO_BIN_EXE_phi");
    let output = Command::new(binary).arg("skill").output()?;
    assert!(output.status.success());
    assert_eq!(output.stdout, include_bytes!("../../../skill/SKILL.md"));
    let dir = tempfile::tempdir()?;
    let output = Command::new(binary)
        .args(["skill", "--install"])
        .arg(dir.path())
        .output()?;
    assert!(output.status.success());
    assert_eq!(
        std::fs::read(dir.path().join("SKILL.md"))?,
        include_bytes!("../../../skill/SKILL.md")
    );
    assert!(
        Command::new(binary)
            .arg("--help")
            .output()?
            .status
            .success()
    );
    let output = Command::new(binary)
        .args(["react", "invalid-id", "claim"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    Ok(())
}
