#![allow(unused_variables)]
#![allow(dead_code)]

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Status {
    Active,
    Archived {
        seqnum_id: Uuid,
        head_seqnum: u64,
        head_realtime: u64,
    },
    Disposed {
        timestamp: u64,
        number: u64,
    },
}

impl Ord for Status {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            // Disposed files come first, sorted by timestamp then number
            (
                Status::Disposed {
                    timestamp: t1,
                    number: n1,
                },
                Status::Disposed {
                    timestamp: t2,
                    number: n2,
                },
            ) => t1.cmp(t2).then_with(|| n1.cmp(n2)),

            // Disposed always comes before non-disposed
            (Status::Disposed { .. }, _) => Ordering::Less,
            (_, Status::Disposed { .. }) => Ordering::Greater,

            // Archived files sorted by head_realtime (then seqnum for stability)
            (
                Status::Archived {
                    seqnum_id: lhs_seqnum_id,
                    head_seqnum: lhs_head_seqnum,
                    head_realtime: lhs_head_realtime,
                },
                Status::Archived {
                    seqnum_id: rhs_seqnum_id,
                    head_seqnum: rhs_head_seqnum,
                    head_realtime: rhs_head_realtime,
                },
            ) => lhs_head_realtime
                .cmp(rhs_head_realtime)
                .then_with(|| lhs_seqnum_id.cmp(rhs_seqnum_id))
                .then_with(|| lhs_head_seqnum.cmp(rhs_head_seqnum)),

            // Archived comes before Active
            (Status::Archived { .. }, Status::Active) => Ordering::Less,
            (Status::Active, Status::Archived { .. }) => Ordering::Greater,

            // Active files are equal in terms of status ordering
            (Status::Active, Status::Active) => Ordering::Equal,
        }
    }
}

impl PartialOrd for Status {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Status {
    /// Parse the journal file status from the end of the path, returning the status and the remaining path
    fn parse(path: &str) -> Option<(Self, &str)> {
        if let Some(stem) = path.strip_suffix(".journal") {
            // Check if it's archived (has @ suffix) or active
            if let Some((prefix, suffix)) = stem.rsplit_once('@') {
                // Parse archived format: @seqnum_id-head_seqnum-head_realtime
                let mut parts = suffix.split('-');

                let seqnum_id = parts.next()?;
                let head_seqnum = parts.next()?;
                let head_realtime = parts.next()?;

                if parts.next().is_some() {
                    return None; // Too many parts
                }

                let seqnum_id = Uuid::try_parse(seqnum_id).ok()?;
                let head_seqnum = u64::from_str_radix(head_seqnum, 16).ok()?;
                let head_realtime = u64::from_str_radix(head_realtime, 16).ok()?;

                Some((
                    Status::Archived {
                        seqnum_id,
                        head_seqnum,
                        head_realtime,
                    },
                    prefix,
                ))
            } else {
                // Active journal
                Some((Status::Active, stem))
            }
        } else if let Some(stem) = path.strip_suffix(".journal~") {
            // Disposed format: @timestamp-number.journal~
            let (prefix, suffix) = stem.rsplit_once('@')?;
            let (timestamp, number) = suffix.rsplit_once('-')?;

            let timestamp = u64::from_str_radix(timestamp, 16).ok()?;
            let number = u64::from_str_radix(number, 16).ok()?;

            Some((Status::Disposed { timestamp, number }, prefix))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    System,
    User(u32),
    Remote(String),
    Unknown(String),
}

impl Source {
    /// Parse the journal basename from the end of the path, returning the basename and the remaining path
    fn parse(path: &str) -> Option<(Self, &str)> {
        // Split on the last '/' to get directory and basename
        let (dir_path, basename) = path.rsplit_once('/')?;

        let journal_type = if basename == "system" {
            Source::System
        } else if let Some(uid_str) = basename.strip_prefix("user-") {
            if let Ok(uid) = uid_str.parse::<u32>() {
                Source::User(uid)
            } else {
                Source::Unknown(basename.to_string())
            }
        } else if let Some(remote_host) = basename.strip_prefix("remote-") {
            Source::Remote(remote_host.to_string())
        } else {
            Source::Unknown(basename.to_string())
        };

        Some((journal_type, dir_path))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Origin {
    pub machine_id: Option<Uuid>,
    pub namespace: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct File {
    pub path: String,
    pub origin: Origin,
    pub status: Status,
}

impl File {
    pub fn from_path(path: &Path) -> Option<Self> {
        Self::from_str(path.to_str()?)
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(path: &str) -> Option<Self> {
        // We only accept an absolute path and we are not lenient when it comes
        // to parsing it.
        assert!(path.starts_with("/"));

        // Parse from right to left
        let (status, path_after_status) = Status::parse(path)?;
        let (source, path_after_source) = Source::parse(path_after_status)?;

        // Try to parse machine ID and namespace from the directory name
        let (machine_id, namespace) = if !path_after_source.is_empty() {
            // Get the last directory component
            let dirname = if let Some((_parent, dir)) = path_after_source.rsplit_once('/') {
                dir
            } else {
                path_after_source
            };

            if let Some((id_str, ns)) = dirname.split_once('.') {
                // Has namespace
                let machine_id = Uuid::try_parse(id_str).ok()?;
                (Some(machine_id), Some(ns.to_string()))
            } else {
                // No namespace, just machine ID
                let machine_id = Uuid::try_parse(dirname).ok();
                (machine_id, None)
            }
        } else {
            (None, None)
        };

        let chain = Origin {
            machine_id,
            namespace,
            source,
        };

        Some(File {
            path: String::from(path),
            origin: chain,
            status,
        })
    }

    pub fn dir(&self) -> &str {
        Path::new(&self.path)
            .parent()
            .and_then(|p| {
                if self.origin.machine_id.is_some() {
                    p.parent()
                } else {
                    Some(p)
                }
            })
            .and_then(|p| p.to_str())
            .expect("A valid UTF-8 directory path")
    }

    /// Check if a path looks like a journal file
    pub fn is_journal_file(path: &str) -> bool {
        path.ends_with(".journal") || path.ends_with(".journal~")
    }

    /// Check if this is an active journal file that's currently being written to
    pub fn is_active(&self) -> bool {
        matches!(self.status, Status::Active)
    }

    /// Check if this is an archived journal file
    pub fn is_archived(&self) -> bool {
        matches!(self.status, Status::Archived { .. })
    }

    /// Check if this is a corrupted/disposed journal file
    pub fn is_disposed(&self) -> bool {
        matches!(self.status, Status::Disposed { .. })
    }

    /// Check if this contains logs from users
    pub fn is_user(&self) -> bool {
        matches!(self.origin.source, Source::User(_))
    }

    /// Check if this contains logs from system
    pub fn is_system(&self) -> bool {
        matches!(self.origin.source, Source::System)
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.origin.source, Source::Remote(_))
    }

    /// Get the user ID if this is a user journal
    pub fn user_id(&self) -> Option<u32> {
        match &self.origin.source {
            Source::User(uid) => Some(*uid),
            _ => None,
        }
    }

    /// Get the remote host if this is a remote journal
    pub fn remote_host(&self) -> Option<&str> {
        match &self.origin.source {
            Source::Remote(host) => Some(host.as_str()),
            _ => None,
        }
    }

    /// Get the namespace if this journal belongs to a namespace
    pub fn namespace(&self) -> Option<&str> {
        self.origin.namespace.as_deref()
    }
}

impl Ord for File {
    fn cmp(&self, other: &Self) -> Ordering {
        // First compare by status, then by path for stability
        self.status
            .cmp(&other.status)
            .then_with(|| self.path.cmp(&other.path))
    }
}

impl PartialOrd for File {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
pub struct Chain {
    // Invariant: the deque is always sorted:
    //  - any disposed files are at the beginning
    //  - any archived files follow with increasing head realtime
    //  - the active file (if any) is at the end
    pub files: VecDeque<File>,
}

impl Chain {
    pub fn new(origin: Origin) -> Self {
        Self {
            files: VecDeque::default(),
        }
    }

    pub fn active_file(&self) -> Option<&File> {
        self.files
            .back()
            .and_then(|f| if f.is_active() { Some(f) } else { None })
    }

    pub fn insert_file(&mut self, file: &File) {
        let pos = self.files.partition_point(|f| f < file);
        self.files.insert(pos, file.clone());
    }

    pub fn remove_file(&mut self, file: &File) {
        // Use partition_point to find where the file would be
        let pos = self.files.partition_point(|f| f < file);

        // Check if the file at this position matches the one we want to remove
        if pos < self.files.len() && &self.files[pos] == file {
            self.files.remove(pos);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Append files that overlap with the time range [start, end) to the provided vector.
    pub fn find_files_in_range(&self, start: u64, end: u64, output: &mut Vec<File>) {
        if self.files.is_empty() || start >= end {
            return;
        }

        let pos = self
            .files
            .partition_point(|f| match &f.status {
                Status::Active => false,
                Status::Archived { head_realtime, .. } => *head_realtime < start,
                Status::Disposed { .. } => true,
            })
            .saturating_sub(1);

        let mut prev_head_realtime = match self.files.get(pos).map(|f| &f.status) {
            Some(Status::Archived { head_realtime, .. }) => Some(*head_realtime),
            _ => None,
        };

        let mut iter = self.files.iter().skip(pos).peekable();

        while let Some(file) = iter.next() {
            match &file.status {
                Status::Archived { head_realtime, .. } => {
                    if *head_realtime >= end {
                        break;
                    }

                    // Peek at the next file to determine tail_realtime
                    let tail_realtime = if let Some(next_file) = iter.peek() {
                        match &next_file.status {
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
                        output.push(file.clone());
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
                        output.push(file.clone());
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

#[derive(Default)]
struct Directory {
    chains: HashMap<Origin, Chain>,
}

#[derive(Default)]
pub struct Registry {
    // Maps a journal directory to the chains it contains
    directories: HashMap<String, Directory>,
}

impl Registry {
    pub fn insert_file(&mut self, file: &File) {
        if let Some(directory) = self.directories.get_mut(file.dir()) {
            if let Some(chain) = directory.chains.get_mut(&file.origin) {
                chain.insert_file(file);
            } else {
                let mut chain = Chain::new(file.origin.clone());
                chain.insert_file(file);
                directory.chains.insert(file.origin.clone(), chain);
            }
        } else {
            let mut chain = Chain::new(file.origin.clone());
            chain.insert_file(file);

            let mut directory = Directory::default();
            directory.chains.insert(file.origin.clone(), chain);

            self.directories.insert(String::from(file.dir()), directory);
        }
    }

    pub fn remove_file(&mut self, file: &File) {
        let mut remove_directory = false;

        if let Some(directory) = self.directories.get_mut(file.dir()) {
            let mut remove_chain = false;

            if let Some(chain) = directory.chains.get_mut(&file.origin) {
                chain.remove_file(file);
                remove_chain = chain.is_empty();
            };

            if remove_chain {
                directory.chains.remove(&file.origin);
            }

            remove_directory = directory.chains.is_empty();
        };

        if remove_directory {
            self.directories.remove(file.dir());
        }
    }

    pub fn find_files_in_range(&self, start: u64, end: u64, output: &mut Vec<File>) {
        for directory in self.directories.values() {
            for chain in directory.chains.values() {
                chain.find_files_in_range(start, end, output);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn create_test_origin() -> Origin {
        Origin {
            machine_id: Some(Uuid::new_v4()),
            namespace: None,
            source: Source::System,
        }
    }

    fn create_archived_file(origin: &Origin, head_realtime: u64) -> File {
        File {
            path: format!("/var/log/journal/system@{}.journal", head_realtime),
            origin: origin.clone(),
            status: Status::Archived {
                seqnum_id: Uuid::new_v4(),
                head_seqnum: 1000 + head_realtime,
                head_realtime,
            },
        }
    }

    fn create_active_file(origin: &Origin) -> File {
        File {
            path: "/var/log/journal/system.journal".to_string(),
            origin: origin.clone(),
            status: Status::Active,
        }
    }

    fn create_disposed_file(origin: &Origin, timestamp: u64, number: u64) -> File {
        File {
            path: format!("/var/log/journal/system@{}-{}.journal~", timestamp, number),
            origin: origin.clone(),
            status: Status::Disposed { timestamp, number },
        }
    }

    #[test]
    fn test_find_files_in_range_empty_chain() {
        let chain = Chain {
            files: VecDeque::new(),
        };

        let mut output = Vec::new();
        chain.find_files_in_range(100, 200, &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn test_find_files_in_range_invalid_range() {
        let origin = create_test_origin();
        let mut chain = Chain {
            files: VecDeque::new(),
        };

        // Add some files
        chain.files.push_back(create_archived_file(&origin, 100));
        chain.files.push_back(create_archived_file(&origin, 200));

        let mut output = Vec::new();
        // Test with start >= end
        chain.find_files_in_range(200, 200, &mut output);
        assert!(output.is_empty());

        chain.find_files_in_range(200, 100, &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn test_find_files_in_range_single_archived() {
        let origin = create_test_origin();
        let mut chain = Chain {
            files: VecDeque::new(),
        };

        // Single archived file with head_realtime = 150
        let file = create_archived_file(&origin, 150);
        chain.files.push_back(file.clone());

        // Test range that starts before and ends after the file
        let mut output = Vec::new();
        chain.find_files_in_range(100, 200, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], file);

        // Test range that starts exactly at head_realtime
        output.clear();
        chain.find_files_in_range(150, 200, &mut output);
        assert_eq!(output.len(), 1);

        // Test range that ends exactly at head_realtime (should not include)
        output.clear();
        chain.find_files_in_range(100, 150, &mut output);
        assert!(output.is_empty());

        // Test range entirely before the file
        output.clear();
        chain.find_files_in_range(50, 100, &mut output);
        assert!(output.is_empty());

        // Test range entirely after (single archived file extends to infinity)
        output.clear();
        chain.find_files_in_range(200, 300, &mut output);
        assert_eq!(output.len(), 1);
    }

    #[test]
    fn test_find_files_in_range_multiple_archived() {
        let origin = create_test_origin();
        let mut chain = Chain {
            files: VecDeque::new(),
        };

        // Multiple archived files: 100, 200, 300, 400
        let file1 = create_archived_file(&origin, 100);
        let file2 = create_archived_file(&origin, 200);
        let file3 = create_archived_file(&origin, 300);
        let file4 = create_archived_file(&origin, 400);

        chain.files.push_back(file1.clone());
        chain.files.push_back(file2.clone());
        chain.files.push_back(file3.clone());
        chain.files.push_back(file4.clone());

        // Range [150, 350) should include files at 100, 200, and 300
        let mut output = Vec::new();
        chain.find_files_in_range(150, 350, &mut output);
        assert_eq!(output.len(), 3);
        assert_eq!(output[0], file1);
        assert_eq!(output[1], file2);
        assert_eq!(output[2], file3);

        // Range [200, 300) should include only file at 200
        output.clear();
        chain.find_files_in_range(200, 300, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], file2);

        // Range [250, 350) should include files at 200 and 300
        output.clear();
        chain.find_files_in_range(250, 350, &mut output);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], file2);
        assert_eq!(output[1], file3);

        // Range [450, 500) should include file at 400 (last file extends to infinity)
        output.clear();
        chain.find_files_in_range(450, 500, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], file4);
    }

    #[test]
    fn test_find_files_in_range_with_active() {
        let origin = create_test_origin();
        let mut chain = Chain {
            files: VecDeque::new(),
        };

        // Archived files at 100, 200, then active
        let file1 = create_archived_file(&origin, 100);
        let file2 = create_archived_file(&origin, 200);
        let active = create_active_file(&origin);

        chain.files.push_back(file1.clone());
        chain.files.push_back(file2.clone());
        chain.files.push_back(active.clone());

        // Range [150, 250) should include files at 100, 200, and active
        let mut output = Vec::new();
        chain.find_files_in_range(150, 250, &mut output);
        assert_eq!(output.len(), 3);
        assert_eq!(output[0], file1);
        assert_eq!(output[1], file2);
        assert_eq!(output[2], active);

        // Range [250, 350) should include only file at 200 and active
        output.clear();
        chain.find_files_in_range(250, 350, &mut output);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], file2);
        assert_eq!(output[1], active);

        // Range [50, 150) should include file at 100
        output.clear();
        chain.find_files_in_range(50, 150, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], file1);
    }

    #[test]
    fn test_find_files_in_range_only_active() {
        let origin = create_test_origin();
        let mut chain = Chain {
            files: VecDeque::new(),
        };

        let active = create_active_file(&origin);
        chain.files.push_back(active.clone());

        // Active file with no archived files should span from u64::MIN to u64::MAX
        let mut output = Vec::new();
        chain.find_files_in_range(0, 100, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], active);

        output.clear();
        chain.find_files_in_range(u64::MAX - 100, u64::MAX, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], active);
    }

    #[test]
    fn test_find_files_in_range_with_disposed() {
        let origin = create_test_origin();
        let mut chain = Chain {
            files: VecDeque::new(),
        };

        // Disposed files should be at the beginning and should be skipped
        let disposed1 = create_disposed_file(&origin, 50, 1);
        let disposed2 = create_disposed_file(&origin, 60, 2);
        let file1 = create_archived_file(&origin, 100);
        let file2 = create_archived_file(&origin, 200);

        chain.files.push_back(disposed1);
        chain.files.push_back(disposed2);
        chain.files.push_back(file1.clone());
        chain.files.push_back(file2.clone());

        // Disposed files should not appear in output
        let mut output = Vec::new();
        chain.find_files_in_range(0, 300, &mut output);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], file1);
        assert_eq!(output[1], file2);
    }

    #[test]
    fn test_find_files_in_range_edge_cases() {
        let origin = create_test_origin();
        let mut chain = Chain {
            files: VecDeque::new(),
        };

        // Files at 100, 200, 300
        let file1 = create_archived_file(&origin, 100);
        let file2 = create_archived_file(&origin, 200);
        let file3 = create_archived_file(&origin, 300);

        chain.files.push_back(file1.clone());
        chain.files.push_back(file2.clone());
        chain.files.push_back(file3.clone());

        // Test exact boundaries
        let mut output = Vec::new();

        // Range [100, 200) should include only file at 100
        chain.find_files_in_range(100, 200, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], file1);

        // Range [200, 300) should include only file at 200
        output.clear();
        chain.find_files_in_range(200, 300, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], file2);

        // Range [300, 400) should include only file at 300
        output.clear();
        chain.find_files_in_range(300, 400, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], file3);

        // Range [199, 201) should include files at 100 and 200
        output.clear();
        chain.find_files_in_range(199, 201, &mut output);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], file1);
        assert_eq!(output[1], file2);
    }

    #[test]
    fn test_find_files_in_range_complex_scenario() {
        let origin = create_test_origin();
        let mut chain = Chain {
            files: VecDeque::new(),
        };

        // Complex scenario with disposed, archived, and active files
        let disposed = create_disposed_file(&origin, 10, 1);
        let file1 = create_archived_file(&origin, 1000);
        let file2 = create_archived_file(&origin, 2000);
        let file3 = create_archived_file(&origin, 3000);
        let file4 = create_archived_file(&origin, 4000);
        let active = create_active_file(&origin);

        chain.files.push_back(disposed);
        chain.files.push_back(file1.clone());
        chain.files.push_back(file2.clone());
        chain.files.push_back(file3.clone());
        chain.files.push_back(file4.clone());
        chain.files.push_back(active.clone());

        // Range [1500, 3500) should include files at 1000, 2000, 3000
        let mut output = Vec::new();
        chain.find_files_in_range(1500, 3500, &mut output);
        assert_eq!(output.len(), 3);
        assert_eq!(output[0], file1);
        assert_eq!(output[1], file2);
        assert_eq!(output[2], file3);

        // Range [4500, 5000) should include active file only
        output.clear();
        chain.find_files_in_range(4500, 5000, &mut output);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], file4);
        assert_eq!(output[1], active);

        // Range [500, 1500) should include file at 1000
        output.clear();
        chain.find_files_in_range(500, 1500, &mut output);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], file1);

        // Range covering everything
        output.clear();
        chain.find_files_in_range(0, u64::MAX, &mut output);
        assert_eq!(output.len(), 5); // All except disposed
        assert_eq!(output[0], file1);
        assert_eq!(output[1], file2);
        assert_eq!(output[2], file3);
        assert_eq!(output[3], file4);
        assert_eq!(output[4], active);
    }
}
