pub mod error;
pub(crate) mod monitor;

pub use crate::registry::error::RegistryError;

use crate::collections::HashSet;
use crate::registry::error::Result;
use crate::repository::{File, Repository, scan_journal_files};
#[cfg(feature = "allocative")]
use allocative::Allocative;

use monitor::Monitor;
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

    pub fn find_files_in_range(&self, start: u64, end: u64) -> HashSet<File> {
        self.repository.find_files_in_range(start, end)
    }

    /// Suggest histogram fields by analyzing cardinality in recent journal files.
    ///
    /// This function examines the last `max_files_per_chain` files in each chain,
    /// computes the cardinality of all fields, and returns fields that have
    /// cardinality between `min_cardinality` and `max_cardinality`.
    ///
    /// # Arguments
    /// * `max_files_per_chain` - Maximum number of recent files to analyze per chain
    /// * `min_cardinality` - Minimum cardinality threshold (fields below this are excluded)
    /// * `max_cardinality` - Maximum cardinality threshold (fields above this are excluded)
    ///
    /// # Returns
    /// A vector of field names that meet the cardinality criteria, sorted by name.
    pub fn suggest_histogram_fields(
        &self,
        max_files_per_chain: usize,
        max_cardinality: usize,
    ) -> Result<Vec<String>> {
        let start = std::time::Instant::now();
        let suggested_fields = self
            .repository
            .suggest_histogram_fields(max_files_per_chain, max_cardinality)?;

        eprintln!("Found suggested fields in {:?}", start.elapsed());

        Ok(suggested_fields)
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
