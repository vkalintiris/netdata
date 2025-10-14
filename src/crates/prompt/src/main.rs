mod commands;
mod completer;
mod shell;

use completer::ShellCompleter;
use rustyline::error::ReadlineError;
use rustyline::{CompletionType, Config, Editor, Result};
use shell::Shell;

fn main() -> Result<()> {
    let shell = Shell::new();

    // Create config with fuzzy completion enabled
    let config = Config::builder()
        .completion_type(CompletionType::Fuzzy)
        .build();

    let helper = ShellCompleter::new(shell.get_command_names());
    let mut rl = Editor::with_config(config)?;
    rl.set_helper(Some(helper));

    if rl.load_history("/tmp/history.txt").is_err() {
        // Ignore if history doesn't exist yet
    }

    println!("Custom Shell - Type 'help' for available commands");

    loop {
        let readline = rl.readline(">> ");
        match readline {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                rl.add_history_entry(line)?;

                // Check for exit command
                if line.split_whitespace().next() == Some("exit") {
                    break;
                }

                // Execute command
                match shell.execute(line) {
                    Ok(_) => {}
                    Err(e) => eprintln!("Error executing command: {}", e),
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("^D");
                break;
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        }
    }

    rl.save_history("/tmp/history.txt")?;
    Ok(())
}
