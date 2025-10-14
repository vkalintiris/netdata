#![allow(unused_imports)]
use clap::Parser;
use std::fs;
use std::io;
use std::path::Path;
use term_grid::{Direction, Filling, Grid, GridOptions};
use terminal_size::{Width, terminal_size};

#[derive(Parser, Debug)]
#[command(
    name = "watch",
    about = "Watch a journal log directory",
    disable_help_flag = true
)]
struct Args {
    #[arg(value_name = "PATH")]
    path: String,

    #[arg(short = 'h', long = "help", action = clap::ArgAction::Help)]
    help: Option<bool>,
}

pub fn execute(args: &[String]) -> io::Result<()> {
    let parsed = match Args::try_parse_from(
        std::iter::once("watch".to_string()).chain(args.iter().cloned()),
    ) {
        Ok(p) => p,
        Err(e) => {
            println!("{}", e);
            return Ok(());
        }
    };

    let path = parsed.path;

    println!("Will watch directory {}", path);

    Ok(())
}
