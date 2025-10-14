use std::collections::HashMap;
use std::io;

/// Command handler function type
pub type CommandHandler = fn(&[String]) -> io::Result<()>;

/// Shell commands registry and executor
pub struct Shell {
    commands: HashMap<String, CommandHandler>,
}

impl Shell {
    /// Create a new shell with all commands registered
    pub fn new() -> Self {
        let mut shell = Shell {
            commands: HashMap::new(),
        };

        // Register built-in commands
        shell.register("clear", crate::commands::clear::execute);
        shell.register("ls", crate::commands::ls::execute);
        shell.register("pwd", crate::commands::pwd::execute);
        shell.register("help", crate::commands::help::execute);
        shell.register("exit", crate::commands::exit::execute);

        shell
    }

    /// Register a new command
    pub fn register(&mut self, name: &str, handler: CommandHandler) {
        self.commands.insert(name.to_string(), handler);
    }

    /// Execute a command line
    pub fn execute(&self, line: &str) -> io::Result<bool> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(true);
        }

        // Parse the line into words, handling quotes
        let args = match shell_words::split(line) {
            Ok(args) => args,
            Err(e) => {
                println!("Error parsing command: {}", e);
                return Ok(true);
            }
        };

        if args.is_empty() {
            return Ok(true);
        }

        let cmd_name = &args[0];
        let cmd_args = args[1..].to_vec();

        if let Some(handler) = self.commands.get(cmd_name) {
            handler(&cmd_args)?;
            Ok(true)
        } else {
            println!("Unknown command: {}. Type 'help' for available commands.", cmd_name);
            Ok(true)
        }
    }

    /// Get list of all registered command names
    pub fn get_command_names(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }
}
