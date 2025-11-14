//! journal-sql standalone binary

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exit_code = journal_sql::run(args);
    std::process::exit(exit_code);
}
