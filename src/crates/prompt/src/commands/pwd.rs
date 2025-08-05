use clap::Parser;
use std::env;
use std::io;

#[derive(Parser, Debug)]
#[command(name = "pwd", about = "Print working directory", disable_help_flag = true)]
struct Args {
    #[arg(short = 'h', long = "help", action = clap::ArgAction::Help)]
    help: Option<bool>,
}

pub fn execute(args: &[String]) -> io::Result<()> {
    match Args::try_parse_from(std::iter::once("pwd".to_string()).chain(args.iter().cloned())) {
        Ok(_) => {
            let current_dir = env::current_dir()?;
            println!("{}", current_dir.display());
            Ok(())
        }
        Err(e) => {
            println!("{}", e);
            Ok(())
        }
    }
}
