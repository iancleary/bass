//! Placeholder binary for the bass command-line client.

fn main() {
    let mut arguments = std::env::args().skip(1);
    let version_requested =
        arguments.next().as_deref() == Some("--version") && arguments.next().is_none();

    if version_requested {
        println!("bass {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    println!(
        "bass is a placeholder command-line client for decision APIs; the command shape is not defined."
    );
}
