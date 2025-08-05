#![allow(unused_imports)]
use clap::Parser;
use journal::file::HashableObject;
use journal::file::Mmap;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use term_grid::{Direction, Filling, Grid, GridOptions};
use terminal_size::{Width, terminal_size};

use journal::file::JournalFileMap;
use journal::index::{FileIndex, FileIndexer};
use journal::registry::RegistryInner;

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

fn collect_fields(journal_file: &JournalFileMap) -> HashSet<String> {
    let mut fields = HashSet::new();

    for item in journal_file.fields() {
        let field = item.unwrap();

        let payload = String::from_utf8_lossy(field.get_payload()).into_owned();
        fields.insert(payload);
    }

    fields
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
    let window_size = 8 * 1024 * 1024;
    let Ok(journal_file) = JournalFileMap::open(&path, window_size) else {
        eprintln!("Failed to open {}", path);
        return Ok(());
    };

    let fields = collect_fields(&journal_file);
    let mut fields: Vec<String> = fields.into_iter().collect();
    fields.sort();

    if true {
        let field_names: Vec<&[u8]> = vec![
            b"log.attributes.method",
            b"resource.attributes.service.name",
            b"log.attributes.protocol",
            b"log.attributes.status",
            b"resource.attributes.service.version",
            b"log.severity_number",
        ];

        let mut file_indexer = FileIndexer::default();
        let file_index = file_indexer
            .index(&journal_file, None, &field_names, 60)
            .unwrap();

        for (idx, (field, rb)) in file_index.entries_index.iter().enumerate() {
            println!("[{}] do: {:#?}, rb: {:#?}", idx, field, rb);
        }

        println!("Index size: {:#?}", file_index.memory_size());
    } else {
        println!("Found {} fields in {}:", fields.len(), path);

        // Get terminal width
        let width = if let Some((Width(w), _)) = terminal_size() {
            w as usize
        } else {
            80 // Default width
        };

        // Create grid using uutils_term_grid
        let grid = Grid::new(
            fields,
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
