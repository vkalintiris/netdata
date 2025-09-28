#![allow(unused_variables)]
#![allow(dead_code)]

use std::cmp::Ordering;
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
                    seqnum_id: lhs_senqum_id,
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
                .then_with(|| lhs_senqum_id.cmp(rhs_seqnum_id))
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

pub struct Chain {
    pub origin: Origin,

    // Invariant: the deque is always contiguous and sorted:
    //  - any disposed files are at the beginning
    //  - any archived files follow with increasing head realtime
    //  - the active file (if any) is at the end
    pub files: std::collections::VecDeque<File>,
}

impl Chain {
    pub fn insert_files(&mut self, files: &[File]) {
        let new_files = files.iter().filter(|f| f.origin == self.origin);

        for file in new_files {
            let pos = self.files.partition_point(|f| f < file);
            self.files.insert(pos, file.clone());
        }

        self.files.make_contiguous();
    }

    pub fn remove_files(&mut self, files: &[File]) {
        // Filter to only files matching this chain's origin
        let removed_files = files.iter().filter(|f| f.origin == self.origin);

        // Remove each matching file
        for file in removed_files {
            // Use partition_point to find where the file would be
            let pos = self.files.partition_point(|f| f < file);

            // Check if the file at this position matches the one we want to remove
            if pos < self.files.len() && &self.files[pos] == file {
                self.files.remove(pos);
            }
        }

        self.files.make_contiguous();
    }

    /// Append files that overlap with the time range [start, end) to the provided vector.
    pub fn find_files_in_range(&self, start: u64, end: u64, output: &mut Vec<File>) {
        if self.files.is_empty() || start >= end {
            return;
        }

        // TODO: collect files whose time range overlaps.
        // NOTE:
        // 1. The tail_realtime of an active file is assumed to be u64::MAX.
        // 2. The head_realtime of an active file is assumed to be:
        //    - the head_realtime of the last seen archived file,
        //    - u64::MIN if there's no last seen archived file.
        // 3. The iterator should not produce any disposed files.
        // 4. The iterator should produce any number (inc. zero) of archived files.
        // 5. Only the last item of the iterator might be an active file.
        // Track the head_realtime of the last archived file we've seen

        let (files, _) = self.files.as_slices();

        let mut iter = files.iter().skip_while(|f| f.is_disposed()).peekable();
        let mut prev_head_realtime: Option<u64> = None;

        while let Some(file) = iter.next() {
            match &file.status {
                Status::Archived { head_realtime, .. } => {
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
                    panic!("Found disposed file: {:#?}", file);
                }
            }
        }
    }
}
