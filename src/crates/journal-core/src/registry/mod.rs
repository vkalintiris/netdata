pub mod error;
pub use error::RegistryError;

use crate::collections::HashSet;
use crate::registry::error::Result;
use crate::repository::{File, Repository, scan_journal_files};
#[cfg(feature = "allocative")]
use allocative::Allocative;

mod monitor;
pub use monitor::Monitor;
use notify::{
    Event,
    event::{EventKind, ModifyKind, RenameMode},
};

#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Registry {
    repository: Repository,
    directories: HashSet<String>,
    #[cfg_attr(feature = "allocative", allocative(skip))]
    monitor: Monitor,
    #[cfg_attr(feature = "allocative", allocative(skip))]
    events: Vec<Event>,
}

impl Registry {
    pub fn new(monitor: Monitor) -> Self {
        Self {
            directories: Default::default(),
            monitor,
            events: Default::default(),
            repository: Default::default(),
        }
    }

    pub fn watch_directory(&mut self, path: &str) -> Result<()> {
        if self.directories.contains(path) {
            return Ok(());
        }

        let files = scan_journal_files(path)?;

        self.monitor.watch_directory(path)?;
        self.directories.insert(String::from(path));

        for file in files {
            self.repository.insert(file)?;
        }

        Ok(())
    }

    pub fn unwatch_directory(&mut self, path: &str) -> Result<()> {
        if !self.directories.contains(path) {
            return Ok(());
        }

        self.monitor.unwatch_directory(path)?;
        self.repository.remove_directory(path);

        Ok(())
    }

    pub fn find_files_in_range(&self, start: u32, end: u32) -> HashSet<File> {
        self.repository.find_files_in_range(start, end)
    }

    pub fn process_events(&mut self) -> Result<()> {
        self.monitor.collect(&mut self.events);

        for event in &self.events {
            match event.kind {
                EventKind::Create(_) => {
                    for path in &event.paths {
                        if let Some(file) = File::from_path(path) {
                            self.repository.insert(file)?;
                        }
                    }
                }
                EventKind::Remove(_) => {
                    for path in &event.paths {
                        if let Some(file) = File::from_path(path) {
                            self.repository.remove(&file)?;
                        }
                    }
                }
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                    // Handle renames: remove old, add new
                    if event.paths.len() >= 2 {
                        if let Some(old_file) = File::from_path(&event.paths[0]) {
                            self.repository.remove(&old_file)?;
                        }
                        if let Some(new_file) = File::from_path(&event.paths[1]) {
                            self.repository.insert(new_file)?;
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
