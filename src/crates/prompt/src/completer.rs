use rustyline::completion::{Completer, Pair};
use rustyline::history::SearchDirection;
use rustyline::{Context, Result};
use rustyline::{Helper, Hinter, Highlighter, Validator};

/// Completer that provides both commands and history for fuzzy search
#[derive(Helper, Hinter, Highlighter, Validator)]
pub struct ShellCompleter {
    commands: Vec<String>,
}

impl ShellCompleter {
    pub fn new(commands: Vec<String>) -> Self {
        ShellCompleter { commands }
    }
}

impl Completer for ShellCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>)> {
        let line = &line[..pos];
        let mut candidates: Vec<Pair> = Vec::new();

        // If we're at the start of the line (completing command name)
        if !line.contains(char::is_whitespace) {
            // Add matching commands
            for cmd in &self.commands {
                if cmd.starts_with(line) {
                    candidates.push(Pair {
                        display: cmd.clone(),
                        replacement: cmd.clone(),
                    });
                }
            }
        }

        // Also add history entries
        let history = ctx.history();
        let mut seen = std::collections::HashSet::new();

        for i in (0..history.len()).rev() {
            if let Ok(Some(entry)) = history.get(i, SearchDirection::Reverse) {
                let entry_str = entry.entry;
                if seen.insert(entry_str.to_string()) {
                    candidates.push(Pair {
                        display: entry_str.to_string(),
                        replacement: entry_str.to_string(),
                    });
                }
            }
        }

        Ok((0, candidates))
    }
}
