//! Journal Tools - Multi-call binary for journalctl and journal-sql
//!
//! This binary can be invoked as either `journalctl` or `journal-sql` depending on
//! how it's called (via symlinks or hardlinks).

use multicall::{MultiCall, ToolContext};

fn main() {
    let mut mc = MultiCall::new();

    // Register tools
    mc.register("journalctl", run_journalctl);
    mc.register("journal-sql", run_journal_sql);

    // Optional: register aliases
    mc.alias("jsql", "journal-sql");
    mc.alias("jctl", "journalctl");

    // Dispatch
    let args: Vec<String> = std::env::args().collect();
    let exit_code = mc.dispatch(&args);
    std::process::exit(exit_code);
}

fn run_journalctl(_ctx: ToolContext, args: Vec<String>) -> i32 {
    // Build full args with tool name as argv[0]
    let mut full_args = vec!["journalctl".to_string()];
    full_args.extend(args);

    journalctl::run(full_args)
}

fn run_journal_sql(_ctx: ToolContext, args: Vec<String>) -> i32 {
    // Build full args with tool name as argv[0]
    let mut full_args = vec!["journal-sql".to_string()];
    full_args.extend(args);

    journal_sql::run(full_args)
}
