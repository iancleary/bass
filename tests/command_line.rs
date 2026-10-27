use std::process::Command;

#[test]
fn prints_hello_world() {
    let output = Command::new(env!("CARGO_BIN_EXE_bass"))
        .output()
        .expect("failed to execute bass");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"hello world\n");
    assert!(output.stderr.is_empty());
}
