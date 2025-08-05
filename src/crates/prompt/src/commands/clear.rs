use clap::Parser;
use std::io::{self, Write};

#[derive(Parser, Debug)]
#[command(name = "clear", about = "Clear the screen", disable_help_flag = true)]
struct Args {
    #[arg(short = 'h', long = "help", action = clap::ArgAction::Help)]
    help: Option<bool>,
}

pub fn execute(args: &[String]) -> io::Result<()> {
    match Args::try_parse_from(std::iter::once("clear".to_string()).chain(args.iter().cloned())) {
        Ok(_) => {
            // ANSI escape sequence to clear screen and move cursor to top-left
            print!("\x1B[2J\x1B[1;1H");
            io::stdout().flush()?;
            Ok(())
        }
        Err(e) => {
            println!("{}", e);
            Ok(())
        }
    }
}
