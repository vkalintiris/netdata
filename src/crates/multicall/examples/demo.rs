//! Demo multi-call binary
//!
//! Build with:
//!   cargo build --example demo
//!
//! Test with:
//!   cd target/debug/examples
//!   ln -s demo tool1
//!   ln -s demo tool2
//!   ln -s demo calc
//!   ./tool1 arg1 arg2
//!   ./tool2
//!   ./calc 2 + 2

use multicall::{MultiCall, ToolContext};
use std::env;

fn main() {
    let mut mc = MultiCall::new();

    // Register tools
    mc.register("tool1", run_tool1);
    mc.register("tool2", run_tool2);
    mc.register("calc", run_calc);

    // Register some aliases
    mc.alias("t1", "tool1");
    mc.alias("calculator", "calc");

    // Dispatch
    let args: Vec<String> = env::args().collect();
    let exit_code = mc.dispatch(&args);
    std::process::exit(exit_code);
}

fn run_tool1(ctx: ToolContext, args: Vec<String>) -> i32 {
    println!("=== Tool 1 ===");
    println!("Invoked as: {}", ctx.invocation_name());
    println!("Tool name: {}", ctx.tool_name());
    println!("Original path: {}", ctx.original_path.display());
    println!("Needs prepend arg: {}", ctx.needs_prepend_arg());
    println!("Arguments: {:?}", args);
    0
}

fn run_tool2(ctx: ToolContext, _args: Vec<String>) -> i32 {
    println!("=== Tool 2 ===");
    println!("Running {} (invoked as {})",
             ctx.tool_name(),
             ctx.invocation_name());
    println!("This tool doesn't take any arguments!");
    0
}

fn run_calc(_ctx: ToolContext, args: Vec<String>) -> i32 {
    println!("=== Calculator ===");

    if args.len() != 3 {
        eprintln!("Usage: calc <num1> <op> <num2>");
        eprintln!("Example: calc 2 + 2");
        return 1;
    }

    let num1: f64 = match args[0].parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("Error: '{}' is not a number", args[0]);
            return 1;
        }
    };

    let num2: f64 = match args[2].parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("Error: '{}' is not a number", args[2]);
            return 1;
        }
    };

    let result = match args[1].as_str() {
        "+" => num1 + num2,
        "-" => num1 - num2,
        "*" | "x" => num1 * num2,
        "/" => {
            if num2 == 0.0 {
                eprintln!("Error: Division by zero");
                return 1;
            }
            num1 / num2
        }
        op => {
            eprintln!("Error: Unknown operator '{}'", op);
            return 1;
        }
    };

    println!("{} {} {} = {}", num1, args[1], num2, result);
    0
}
