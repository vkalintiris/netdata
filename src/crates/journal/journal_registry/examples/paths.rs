#![allow(unused_variables)]
#![allow(dead_code)]

use uuid::Uuid;

#[derive(Debug, Clone, Copy)]
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

fn strip_journal_file_status<'a>(jfs: &JournalFileStatus, filename: &'a str) -> Option<&'a str> {
    match jfs {
        JournalFileStatus::Active => {
            // Active files: just remove .journal
            filename.strip_suffix(".journal")
        }
        JournalFileStatus::Archived { .. } | JournalFileStatus::Disposed { .. } => {
            // Everything before the last at-symbol
            filename
                .rsplit_once('@')
                .map(|(basename, _suffix)| basename)
        }
    }
}

impl JournalFileStatus {
    fn from_path(path: &str) -> Option<Self> {
        if let Some(stem) = path.strip_suffix(".journal") {
            let Some((stem_prefix, stem_suffix)) = stem.rsplit_once('@') else {
                return Some(JournalFileStatus::Active);
            };

            let mut parts = stem_suffix.split('-');

            let seqnum_id = parts.next()?;
            let head_seqnum = parts.next()?;
            let head_realtime = parts.next()?;

            if parts.next().is_some() {
                return None;
            }

            let seqnum_id = Uuid::try_parse(seqnum_id).ok()?;
            let head_seqnum = u64::from_str_radix(head_seqnum, 16).ok()?;
            let head_realtime = u64::from_str_radix(head_realtime, 16).ok()?;

            return Some(JournalFileStatus::Archived {
                seqnum_id,
                head_seqnum,
                head_realtime,
            });
        } else if let Some(stem) = path.strip_suffix(".journal~") {
            let (prefix, suffix) = stem.rsplit_once('@')?;

            let (timestamp, number) = suffix.rsplit_once('-')?;

            let timestamp = match u64::from_str_radix(timestamp, 16) {
                Ok(ts) => ts,
                Err(e) => {
                    return None;
                }
            };

            let number = match u64::from_str_radix(number, 16) {
                Ok(num) => num,
                Err(e) => {
                    return None;
                }
            };

            return Some(JournalFileStatus::Disposed { timestamp, number });
        }

        None
    }
}

#[derive(Debug, Clone)]
enum JournalBasename {
    System,
    User(u32),
    Remote(String),
    Unkonwn(String),
}

impl JournalBasename {
    fn new(jfs: &JournalFileStatus, path: &str) -> Option<Self> {
        let basename = strip_journal_file_status(jfs, path)?;

        let (_, basename) = basename.rsplit_once('/')?;

        if basename.ends_with("system") {
            return Some(JournalBasename::System);
        }

        if let Some(uid_str) = basename.strip_suffix("user-") {
            if let Ok(uid) = uid_str.parse::<u32>() {
                return Some(JournalBasename::User(uid));
            }
        }

        if let Some(remote_host) = basename.strip_prefix("remote-") {
            return Some(JournalBasename::Remote(remote_host.to_string()));
        }

        Some(JournalBasename::Unkonwn(String::from(basename)))
    }
}

fn strip_journal_base_name<'a>(
    jbs: &JournalBasename,
    jfs: &JournalFileStatus,
    path: &'a str,
) -> Option<&'a str> {
    let path = strip_journal_file_status(jfs, path)?;
    println!("Tsimpa....... {:#?}", path);

    println!("The fuck?");
    match jbs {
        JournalBasename::System => {
            let r = path.strip_suffix("system");
            println!("r: {:#?}, path: {:#?}", r, path);
            return r;
        }
        JournalBasename::User(uid) => path.strip_suffix(format!("user-{}", uid).as_str()),
        JournalBasename::Remote(remote) => path.strip_suffix(remote.as_str()),
        JournalBasename::Unkonwn(unknown) => path.strip_suffix(unknown.as_str()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MachineNamespace {
    machine_id: Uuid,
    namespace: Option<String>,
}

impl MachineNamespace {
    fn new(jbs: &JournalBasename, jfs: &JournalFileStatus, path: &str) -> Option<Self> {
        use std::path::{Path, PathBuf};

        let p = Path::new(path);
        let dirname = p
            .parent()
            .and_then(|x| x.file_name())
            .and_then(|x| x.to_str())?;

        if let Some((id_str, ns)) = dirname.split_once('.') {
            // Has namespace
            let machine_id = Uuid::try_parse(id_str).ok()?;
            Some(MachineNamespace {
                machine_id,
                namespace: Some(ns.to_string()),
            })
        } else {
            // No namespace, just machine ID
            let machine_id = Uuid::try_parse(dirname).ok()?;
            Some(MachineNamespace {
                machine_id,
                namespace: None,
            })
        }
    }
}

fn main() {
    // let path = "/var/log/journal/dd6fe19058f643f9bd46d5d3aafa8c0e/user-1000@00062f970122eeee-c3edad506a68f0fd.journal~";
    // let path = "/var/log/journal/dd6fe19058f643f9bd46d5d3aafa8c0e.netdata/system@3a5ff40d19de4cfab05abfec1d132479-00000000010dcee9-00062f669854c4d3.journal";

    // let path = "/var/log/remote/remote-10.20.1.98@1c510f67f51d4ebbb61e96571bfb8967-0000000000b13cb6-00063f6d9d99c2d8.journal";
    let path = "/var/log/journal/dd6fe19058f643f9bd46d5d3aafa8c0e.netdata/system@3a5ff40d19de4cfab05abfec1d132479-00000000010dcee9-00062f669854c4d3.journal";
    let jfs = JournalFileStatus::from_path(path).unwrap();
    let jbs = JournalBasename::new(&jfs, path).unwrap();

    let mn = MachineNamespace::new(&jbs, &jfs, path);

    println!("{:#?}", path);
    println!("jfs: {:#?}", jfs);
    println!("jbs: {:#?}", jbs);
    println!("mn: {:#?}", mn);
}
