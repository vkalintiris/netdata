use crate::collections::{HashMap, VecDeque};
use crate::repository::error::Result;
use crate::repository::file::{File, Origin, Status};
#[cfg(feature = "allocative")]
use allocative::Allocative;

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Chain {
    // Invariant: the deque is always sorted:
    //  - any disposed files are at the beginning
    //  - any archived files follow with increasing head realtime
    //  - the active file (if any) is at the end
    pub files: VecDeque<File>,
}

impl Chain {
    #[allow(dead_code)]
    pub fn active_file(&self) -> Option<&File> {
        self.files
            .back()
            .and_then(|f| if f.is_active() { Some(f) } else { None })
    }

    pub fn insert_file(&mut self, file: File) {
        let pos = self.files.partition_point(|f| *f < file);

        if pos < self.files.len() && self.files[pos] == file {
            return;
        }

        self.files.insert(pos, file.clone());
    }

    pub fn remove_file(&mut self, file: &File) {
        // Use partition_point to find where the file would be
        let pos = self.files.partition_point(|f| f < file);

        // Check if the file at this position matches the one we want to remove
        if pos < self.files.len() && self.files[pos] == *file {
            self.files.remove(pos);
        }
    }

    pub fn pop_front(&mut self) -> Option<File> {
        self.files.pop_front()
    }

    pub fn back(&self) -> Option<&File> {
        self.files.back()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn drain(&mut self, cutoff_time: u64) -> impl Iterator<Item = File> + '_ {
        let pos = self.files.partition_point(|file| match file.status() {
            Status::Active => false,
            Status::Archived { head_realtime, .. } => *head_realtime <= cutoff_time,
            Status::Disposed { timestamp, .. } => *timestamp <= cutoff_time,
        });

        self.files.drain(..pos)
    }

    /// Find files that overlap with the time range [start, end)
    ///
    /// Extends the provided collection with matching files. This allows flexibility
    /// in choosing the collection type:
    /// - `HashSet<File>` - for unique files (no duplicates)
    /// - `Vec<File>` - for ordered files (may have duplicates if called multiple times)
    /// - Any type implementing `Extend<File>`
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Into a HashSet
    /// let mut files = HashSet::new();
    /// chain.find_files_in_range(100, 200, &mut files);
    ///
    /// // Into a Vec
    /// let mut files = Vec::new();
    /// chain.find_files_in_range(100, 200, &mut files);
    /// ```
    pub fn find_files_in_range<C>(&self, start: u32, end: u32, files: &mut C)
    where
        C: Extend<File>,
    {
        if self.files.is_empty() || start >= end {
            return;
        }

        const USEC_PER_SEC: u64 = std::time::Duration::from_secs(1).as_micros() as u64;
        let start = start as u64 * USEC_PER_SEC;
        let end = end as u64 * USEC_PER_SEC;

        let pos = self
            .files
            .partition_point(|f| match f.status() {
                Status::Active => false,
                Status::Archived { head_realtime, .. } => *head_realtime < start,
                Status::Disposed { .. } => true,
            })
            .saturating_sub(1);

        let mut prev_head_realtime = match self.files.get(pos).map(|f| f.status()) {
            Some(Status::Archived { head_realtime, .. }) => Some(*head_realtime),
            _ => None,
        };

        let mut iter = self.files.iter().skip(pos).peekable();

        while let Some(file) = iter.next() {
            match file.status() {
                Status::Archived { head_realtime, .. } => {
                    if *head_realtime >= end {
                        break;
                    }

                    // Peek at the next file to determine tail_realtime
                    let tail_realtime = if let Some(next_file) = iter.peek() {
                        match next_file.status() {
                            Status::Active => {
                                // We don't know the tail_realtime of the active file
                                u64::MAX
                            }
                            Status::Archived {
                                head_realtime: tail_realtime,
                                ..
                            } => *tail_realtime,
                            Status::Disposed { .. } => {
                                // This shouldn't happen given our ordering, but handle it
                                panic!(
                                    "Tried to lookup tail_realtime of disposed file: {:#?}",
                                    next_file
                                );
                            }
                        }
                    } else {
                        // This is the last file and it's archived
                        u64::MAX
                    };

                    // Check if [head_realtime, tail_realtime) overlaps with [start, end)
                    // Overlap occurs when: head_realtime < end && tail_realtime > start
                    if *head_realtime < end && tail_realtime > start {
                        files.extend(std::iter::once(file.clone()));
                    }

                    // Remember this head_realtime for potential active file
                    prev_head_realtime = Some(*head_realtime);
                }
                Status::Active => {
                    // For active files:
                    // - tail_realtime is assumed to be u64::MAX (still being written)
                    // - head_realtime is either the previous archived file's head_realtime or u64::MIN

                    let head_realtime = prev_head_realtime.unwrap_or(u64::MIN);
                    let tail_realtime = u64::MAX;

                    // Check overlap: active_head < end && active_tail > start
                    if head_realtime < end && tail_realtime > start {
                        files.extend(std::iter::once(file.clone()));
                    }

                    // There should only be one active file at the end
                    break;
                }
                Status::Disposed { .. } => {
                    // This might happen if the partition point moved
                    // us in a disposed file position.
                    continue;
                }
            }
        }
    }
}

#[derive(Default, Debug)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub(super) struct Directory {
    pub(super) chains: HashMap<Origin, Chain>,
}

#[derive(Default)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Repository {
    // Maps a journal directory to the chains it contains
    pub(super) directories: HashMap<String, Directory>,
}

impl Repository {
    pub fn insert_file(&mut self, file: File) -> Result<()> {
        let dir = file.dir()?.to_string();

        if let Some(directory) = self.directories.get_mut(&dir) {
            if let Some(chain) = directory.chains.get_mut(file.origin()) {
                chain.insert_file(file);
            } else {
                let origin = file.origin().clone();
                let mut chain = Chain::default();
                chain.insert_file(file);
                directory.chains.insert(origin, chain);
            }
        } else {
            let origin = file.origin().clone();
            let mut chain = Chain::default();
            chain.insert_file(file);

            let mut directory = Directory::default();
            directory.chains.insert(origin, chain);

            self.directories.insert(dir, directory);
        }
        Ok(())
    }

    pub fn remove_file(&mut self, file: &File) -> Result<()> {
        let dir = file.dir()?;
        let mut remove_directory = false;

        if let Some(directory) = self.directories.get_mut(dir) {
            let mut remove_chain = false;

            if let Some(chain) = directory.chains.get_mut(file.origin()) {
                chain.remove_file(file);
                remove_chain = chain.is_empty();
            };

            if remove_chain {
                directory.chains.remove(file.origin());
            }

            remove_directory = directory.chains.is_empty();
        };

        if remove_directory {
            self.directories.remove(dir);
        }
        Ok(())
    }

    pub fn remove_directory_files(&mut self, path: &str) {
        self.directories.remove(path);
    }

    pub fn rename_file(&mut self, from: &File, to: File) -> Result<()> {
        self.remove_file(from)?;
        self.insert_file(to)?;
        Ok(())
    }

    /// Find all files across all directories and chains that overlap with the time range [start, end)
    ///
    /// Returns a collection of your choice using type inference, similar to `Iterator::collect()`.
    /// The compiler determines the return type based on how you use the result.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // Into a HashSet (for unique files, no duplicates across chains)
    /// let files: HashSet<File> = repository.find_files_in_range(100, 200);
    ///
    /// // Into a Vec (for ordered files)
    /// let files: Vec<File> = repository.find_files_in_range(100, 200);
    ///
    /// // Type inference works too
    /// let files = repository.find_files_in_range::<HashSet<_>>(100, 200);
    /// ```
    pub fn find_files_in_range<C>(&self, start: u32, end: u32) -> C
    where
        C: FromIterator<File> + Extend<File> + Default,
    {
        let mut files = C::default();

        for directory in self.directories.values() {
            for chain in directory.chains.values() {
                chain.find_files_in_range(start, end, &mut files);
            }
        }

        files
    }

    /// Suggest histogram fields by analyzing cardinality in recent journal files.
    ///
    /// This iterates through all chains, examines the last N files per chain,
    /// and computes field cardinality to identify good candidates for histograms.
    pub fn suggest_histogram_fields(
        &self,
        max_files_per_chain: usize,
        max_cardinality: usize,
    ) -> Result<Vec<String>> {
        use crate::file::{JournalFile, Mmap};
        use std::collections::HashMap;

        // Aggregate cardinality across all files
        let mut field_cardinalities: HashMap<String, usize> = HashMap::default();

        // Iterate through all chains
        for directory in self.directories.values() {
            eprintln!("Checking directory...");

            for chain in directory.chains.values() {
                eprintln!("Checking chain...");

                // Get the last N files from this chain (files are already sorted)
                let files_to_analyze = chain
                    .files
                    .iter()
                    .rev()
                    .take(max_files_per_chain)
                    .collect::<Vec<_>>();

                for file in files_to_analyze {
                    println!("Checking file: {:#?}", file.path());

                    // Open the journal file
                    let window_size = 8 * 1024 * 1024; // 8 MiB
                    let journal_file = match JournalFile::<Mmap>::open(file, window_size) {
                        Ok(jf) => jf,
                        Err(_e) => {
                            // Skip files we can't open
                            continue;
                        }
                    };

                    // Iterate through all fields in this file
                    for field_result in journal_file.fields() {
                        let field_name = {
                            let Ok(field) = field_result else {
                                continue;
                            };

                            // Skip fields with names >= 64 bytes
                            if field.payload.len() >= 64 {
                                continue;
                            }

                            // Convert field name to string using lossy conversion
                            let Ok(field_name) = String::from_utf8(field.payload.to_vec()) else {
                                // Skip fields that don't contain valid utf-8
                                continue;
                            };

                            field_name
                        };

                        // Iterate through all data objects for this field
                        let mut iter = match journal_file.field_data_objects(field_name.as_bytes())
                        {
                            Ok(iter) => iter,
                            Err(_) => continue,
                        };

                        let mut cardinality = 0;

                        while let Some(Ok(data)) = iter.next() {
                            let payload = data.payload_bytes();

                            if payload.len() > (64 + 128) {
                                cardinality = usize::MAX;
                                continue;
                            }

                            if str::from_utf8(payload).is_err() {
                                cardinality = usize::MAX;
                                break;
                            }

                            cardinality += 1;

                            if cardinality > max_cardinality {
                                // We won't use this field
                                cardinality = usize::MAX;
                                break;
                            }
                        }

                        if cardinality > 0 && cardinality <= max_cardinality {
                            if let Some(c) = field_cardinalities.get_mut(&field_name) {
                                *c = cardinality;
                            } else {
                                field_cardinalities.insert(field_name, cardinality);
                            }
                        }
                    }
                }
            }
        }

        // eprintln!("field cardinalities: {:#?}", field_cardinalities);

        // Filter fields by cardinality and return sorted list
        let mut result: Vec<String> = field_cardinalities
            .into_iter()
            .filter_map(|(field_name, cardinality)| {
                if cardinality <= max_cardinality {
                    Some(field_name)
                } else {
                    None
                }
            })
            .collect();

        // eprintln!("Result: {:#?}", result);

        result.sort();
        Ok(result)
    }
}
