fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 1 && arguments[0] == "--version" {
        println!("bass {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    println!("hello world");
}
