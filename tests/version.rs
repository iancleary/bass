use std::process::Command;

#[test]
fn version_flag_prints_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_bass"))
        .arg("--version")
        .output()
        .expect("failed to execute bass");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("stdout is not utf-8");
    let expected = format!("bass {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(stdout, expected);
}
