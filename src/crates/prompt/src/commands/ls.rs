use clap::Parser;
use std::fs;
use std::io;
use std::path::Path;
use term_grid::{Direction, Filling, Grid, GridOptions};
use terminal_size::{terminal_size, Width};

#[derive(Parser, Debug)]
#[command(name = "ls", about = "List directory contents", disable_help_flag = true)]
struct Args {
    /// Path to list (default: current directory)
    #[arg(value_name = "PATH")]
    path: Option<String>,

    /// Show hidden files
    #[arg(short = 'a', long = "all")]
    all: bool,

    /// Long format
    #[arg(short = 'l', long = "long")]
    long: bool,

    #[arg(short = 'h', long = "help", action = clap::ArgAction::Help)]
    help: Option<bool>,
}

pub fn execute(args: &[String]) -> io::Result<()> {
    let parsed = match Args::try_parse_from(std::iter::once("ls".to_string()).chain(args.iter().cloned())) {
        Ok(p) => p,
        Err(e) => {
            println!("{}", e);
            return Ok(());
        }
    };

    let path = parsed.path.as_deref().unwrap_or(".");
    let path = Path::new(path);

    // Read directory entries
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("ls: cannot access '{}': {}", path.display(), e);
            return Ok(());
        }
    };

    let mut names: Vec<String> = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("Error reading entry: {}", e);
                continue;
            }
        };

        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().to_string();

        // Skip hidden files unless -a is specified
        if !parsed.all && name.starts_with('.') {
            continue;
        }

        // Add color/suffix indicators for directories
        let metadata = entry.metadata();
        let display_name = if let Ok(meta) = metadata {
            if meta.is_dir() {
                format!("\x1b[1;34m{}/\x1b[0m", name) // Blue and bold for directories
            } else if meta.is_symlink() {
                format!("\x1b[1;36m{}\x1b[0m", name) // Cyan for symlinks
            } else {
                name
            }
        } else {
            name
        };

        if parsed.long {
            // Long format: show permissions, size, etc.
            if let Ok(meta) = entry.metadata() {
                let size = meta.len();
                let file_type = if meta.is_dir() { "d" } else { "-" };
                println!("{} {:>10} {}", file_type, size, display_name);
            }
        } else {
            names.push(display_name);
        }
    }

    // If not long format, use grid display
    if !parsed.long {
        // Sort names
        names.sort();

        // Get terminal width
        let width = if let Some((Width(w), _)) = terminal_size() {
            w as usize
        } else {
            80 // Default width
        };

        // Create grid using uutils_term_grid
        let grid = Grid::new(
            names,
            GridOptions {
                filling: Filling::Spaces(2),
                direction: Direction::LeftToRight,
                width,
            },
        );

        print!("{}", grid);
    }

    Ok(())
}
