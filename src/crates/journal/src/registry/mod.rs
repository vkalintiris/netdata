pub mod cache;
pub mod error;
pub(crate) mod monitor;

pub use crate::registry::error::RegistryError;

use crate::registry::error::Result;
use crate::repository::{File, Repository, scan_journal_files};
use std::collections::HashSet;

use monitor::Monitor;
use notify::{
    Event,
    event::{EventKind, ModifyKind, RenameMode},
};

pub struct Registry {
    repository: Repository,
    directories: HashSet<String>,
    monitor: Monitor,
    events: Vec<Event>,
}

impl Registry {
    pub fn new() -> Result<Self> {
        Ok(Self {
            directories: Default::default(),
            monitor: Monitor::new()?,
            events: Default::default(),
            repository: Default::default(),
        })
    }

    pub fn watch_directory(&mut self, path: &str) -> Result<()> {
        if self.directories.contains(path) {
            return Ok(());
        }

        let files = scan_journal_files(path)?;

        self.monitor.watch_directory(path)?;
        self.directories.insert(String::from(path));

        for file in files {
            self.repository.insert_file(file)?;
        }

        Ok(())
    }

    pub fn unwatch_directory(&mut self, path: &str) -> Result<()> {
        if !self.directories.contains(path) {
            return Ok(());
        }

        self.monitor.unwatch_directory(path)?;
        self.repository.remove_directory_files(path);

        Ok(())
    }

    pub fn find_files_in_range(&self, start: u64, end: u64, output: &mut Vec<File>) {
        self.repository.find_files_in_range(start, end, output);
    }

    pub fn process_events(&mut self) -> Result<()> {
        self.monitor.collect(&mut self.events);

        for event in &self.events {
            match event.kind {
                EventKind::Create(_) => {
                    for path in &event.paths {
                        if let Some(file) = File::from_path(path) {
                            self.repository.insert_file(file)?;
                        }
                    }
                }
                EventKind::Remove(_) => {
                    for path in &event.paths {
                        if let Some(file) = File::from_path(path) {
                            self.repository.remove_file(&file)?;
                        }
                    }
                }
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                    // Handle renames: remove old, add new
                    if event.paths.len() >= 2 {
                        if let Some(old_file) = File::from_path(&event.paths[0]) {
                            self.repository.remove_file(&old_file)?;
                        }
                        if let Some(new_file) = File::from_path(&event.paths[1]) {
                            self.repository.insert_file(new_file)?;
                        }
                    } else {
                        eprintln!("Uncaught rename event: {:#?}", event.paths);
                    }
                }
                EventKind::Modify(ModifyKind::Name(rename_mode)) => {
                    eprintln!("Unhandled rename mode: {:#?}", rename_mode);
                }
                _ => {} // Ignore other events
            }
        }
        Ok(())
    }
}
