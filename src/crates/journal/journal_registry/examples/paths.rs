#![allow(unused_variables)]
#![allow(dead_code)]

use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum JournalFileStatus {
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

impl JournalFileStatus {
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
                    JournalFileStatus::Archived {
                        seqnum_id,
                        head_seqnum,
                        head_realtime,
                    },
                    prefix,
                ))
            } else {
                // Active journal
                Some((JournalFileStatus::Active, stem))
            }
        } else if let Some(stem) = path.strip_suffix(".journal~") {
            // Disposed format: @timestamp-number.journal~
            let (prefix, suffix) = stem.rsplit_once('@')?;
            let (timestamp, number) = suffix.rsplit_once('-')?;

            let timestamp = u64::from_str_radix(timestamp, 16).ok()?;
            let number = u64::from_str_radix(number, 16).ok()?;

            Some((JournalFileStatus::Disposed { timestamp, number }, prefix))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum JournalSource {
    System,
    User(u32),
    Remote(String),
    Unknown(String),
}

impl JournalSource {
    /// Parse the journal basename from the end of the path, returning the basename and the remaining path
    fn parse(path: &str) -> Option<(Self, &str)> {
        // Split on the last '/' to get directory and basename
        let (dir_path, basename) = path.rsplit_once('/')?;

        let journal_type = if basename == "system" {
            JournalSource::System
        } else if let Some(uid_str) = basename.strip_prefix("user-") {
            if let Ok(uid) = uid_str.parse::<u32>() {
                JournalSource::User(uid)
            } else {
                JournalSource::Unknown(basename.to_string())
            }
        } else if let Some(remote_host) = basename.strip_prefix("remote-") {
            JournalSource::Remote(remote_host.to_string())
        } else {
            JournalSource::Unknown(basename.to_string())
        };

        Some((journal_type, dir_path))
    }
}

/// Parse a journal file path into its components
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct JournalFileInfo {
    path: String,
    status: JournalFileStatus,
    source: JournalSource,
    machine_id: Option<Uuid>,
    namespace: Option<String>,
}

impl JournalFileInfo {
    fn parse(path: &str) -> Option<Self> {
        // We only accept an absolute path and we are not lenient when it comes
        // to parsing it.
        assert!(path.starts_with("/"));

        // Parse from right to left
        let (status, path_after_status) = JournalFileStatus::parse(path)?;
        let (source, path_after_source) = JournalSource::parse(path_after_status)?;

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

        Some(JournalFileInfo {
            path: String::from(path),
            status,
            source,
            machine_id,
            namespace,
        })
    }

    /// Check if this is an active journal file that's currently being written to
    pub fn is_active(&self) -> bool {
        matches!(self.status, JournalFileStatus::Active)
    }

    /// Check if this is a corrupted/disposed journal file
    pub fn is_disposed(&self) -> bool {
        matches!(self.status, JournalFileStatus::Disposed { .. })
    }
}

fn main() {
    let test_paths = vec![
        "/var/log/journal/dd6fe19058f643f9bd46d5d3aafa8c0e/user-1000@00062f970122eeee-c3edad506a68f0fd.journal~",
        "/var/log/journal/dd6fe19058f643f9bd46d5d3aafa8c0e.netdata/system@3a5ff40d19de4cfab05abfec1d132479-00000000010dcee9-00062f669854c4d3.journal",
        "/var/log/remote/remote-10.20.1.98@1c510f67f51d4ebbb61e96571bfb8967-0000000000b13cb6-00063f6d9d99c2d8.journal",
        "/var/log/journal/dd6fe19058f643f9bd46d5d3aafa8c0e/system.journal",
        "/var/log/journal/system.journal",
    ];

    for path in test_paths {
        println!("\nParsing: {}", path);

        if let Some(parsed) = JournalFileInfo::parse(path) {
            println!("  Path: {:?}", parsed.path);
            println!("  Status: {:?}", parsed.status);
            println!("  Source: {:?}", parsed.source);
            println!("  Machine ID: {:?}", parsed.machine_id);
            println!("  Namespace: {:?}", parsed.namespace);
        } else {
            println!("  Failed to parse!");
        }
    }
}
