#![allow(unused_variables)]
#![allow(dead_code)]

use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Status {
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
pub(crate) enum Source {
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
pub(crate) struct Origin {
    pub machine_id: Option<Uuid>,
    pub namespace: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct JournalFile {
    pub path: String,
    pub origin: Origin,
    pub status: Status,
}

impl JournalFile {
    pub fn parse(path: &str) -> Option<Self> {
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

        Some(JournalFile {
            path: String::from(path),
            origin: chain,
            status,
        })
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
