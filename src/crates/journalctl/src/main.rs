//! journalctl standalone binary

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exit_code = journalctl::run(args);
    std::process::exit(exit_code);
}
