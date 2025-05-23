#![allow(dead_code)]

use chrono::{DateTime, TimeZone, Utc};
use error::{JournalError, Result};
use journal_reader::{journal_filter, Direction, JournalReader, Location};
use object_file::*;
use std::collections::HashMap;
use window_manager::MemoryMap;

pub struct EntryData {
    pub offset: u64,
    pub realtime: u64,
    pub monotonic: u64,
    pub boot_id: String,
    pub seqnum: u64,
    pub fields: Vec<(String, String)>,
}

impl std::fmt::Debug for EntryData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Start with a custom struct name
        write!(f, "@{:#x?} {{ fields: [", self.offset)?;

        // Iterate through fields and format each one
        for (i, (key, value)) in self.fields.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "({:?}, {:?})", key, value)?;
        }

        // Close the formatting
        write!(f, "] }}")
    }
}

impl EntryData {
    /// Extract all data from an entry into an owned structure
    pub fn from_offset<M: MemoryMap>(
        object_file: &ObjectFile<M>,
        entry_offset: u64,
    ) -> Result<EntryData> {
        // Get the entry object
        let entry_object = object_file.entry_object(entry_offset)?;

        // Extract basic information from the entry header
        let realtime = entry_object.header.realtime;
        let monotonic = entry_object.header.monotonic;
        let boot_id = format_uuid_bytes(&entry_object.header.boot_id);
        let seqnum = entry_object.header.seqnum;

        drop(entry_object);

        // Create a vector to hold all fields
        let mut fields = Vec::new();

        // Iterate through all data objects for this entry
        for data_result in object_file.entry_data_objects(entry_offset)? {
            let data_object = data_result?;
            let payload = data_object.payload_bytes();

            // Find the first '=' character to split field and value
            if let Some(equals_pos) = payload.iter().position(|&b| b == b'=') {
                let field = String::from_utf8_lossy(&payload[0..equals_pos]).to_string();
                let value = String::from_utf8_lossy(&payload[equals_pos + 1..]).to_string();

                if field.starts_with("_") {
                    continue;
                }

                fields.push((field, value));
            }
        }

        // Create and return the EntryData struct
        Ok(EntryData {
            offset: entry_offset,
            realtime,
            monotonic,
            boot_id,
            seqnum,
            fields,
        })
    }

    pub fn get_field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn datetime(&self) -> DateTime<Utc> {
        let seconds = (self.realtime / 1_000_000) as i64;
        let nanoseconds = ((self.realtime % 1_000_000) * 1000) as u32;
        Utc.timestamp_opt(seconds, nanoseconds).unwrap()
    }
}

fn format_uuid_bytes(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

// fn create_logs() {
//     let mut logger = journal_logger::JournalLogger::new(
//         "/home/vk/opt/sd/netdata/usr/sbin/log2journal",
//         "/home/vk/opt/sd/netdata/usr/sbin/systemd-cat-native",
//     );

//     for i in 0..5 {
//         logger.add_field("SVD_1", "svd-1");
//         logger.add_field("MESSAGE", &format!("svd-1-iteration-{}", i));
//         logger.flush().unwrap();

//         logger.add_field("SVD_1", "svd-a");
//         logger.add_field("MESSAGE", &format!("svd-1-iteration-{}", i));
//         logger.flush().unwrap();

//         logger.add_field("SVD_2", "svd-2");
//         logger.add_field("MESSAGE", &format!("svd-2-iteration-{}", i));
//         logger.flush().unwrap();

//         logger.add_field("SVD_2", "svd-b");
//         logger.add_field("MESSAGE", &format!("svd-2-iteration-{}", i));
//         logger.flush().unwrap();

//         logger.add_field("SVD_3", "svd-3");
//         logger.add_field("MESSAGE", &format!("svd-3-iteration-{}", i));
//         logger.flush().unwrap();

//         logger.add_field("SVD_3", "svd-c");
//         logger.add_field("MESSAGE", &format!("svd-3-iteration-{}", i));
//         logger.flush().unwrap();

//         logger.add_field("SVD_1", "svd-1");
//         logger.add_field("SVD_2", "svd-2");
//         logger.add_field("MESSAGE", &format!("svd-12-iteration-{}", i));
//         logger.flush().unwrap();

//         logger.add_field("SVD_1", "svd-1");
//         logger.add_field("SVD_3", "svd-3");
//         logger.add_field("MESSAGE", &format!("svd-13-iteration-{}", i));
//         logger.flush().unwrap();

//         logger.add_field("SVD_2", "svd-2");
//         logger.add_field("SVD_3", "svd-3");
//         logger.add_field("MESSAGE", &format!("svd-23-iteration-{}", i));
//         logger.flush().unwrap();

//         logger.add_field("SVD_1", "svd-1");
//         logger.add_field("SVD_2", "svd-2");
//         logger.add_field("SVD_3", "svd-3");
//         logger.add_field("MESSAGE", &format!("svd-123-iteration-{}", i));
//         logger.flush().unwrap();

//         std::thread::sleep(std::time::Duration::from_secs(1));
//     }
// }

// fn test_head<M: MemoryMap>(object_file: &ObjectFile<M>) -> Result<()> {
//     let mut jr = JournalReader::default();
//     assert!(!jr.step(object_file, Direction::Backward)?);

//     match jr.get_realtime_usec(object_file).expect_err("unset cursor") {
//         JournalError::UnsetCursor => {}
//         _ => {
//             panic!("unexpected journal error");
//         }
//     };

//     assert!(jr.step(object_file, Direction::Forward)?);
//     let rt1a = jr.get_realtime_usec(object_file).expect("head's realtime");
//     assert_eq!(rt1a, object_file.journal_header().head_entry_realtime);

//     assert!(jr.step(object_file, Direction::Forward)?);
//     let rt2 = jr.get_realtime_usec(object_file).expect("2nd realtime");
//     assert_ne!(rt2, rt1a);

//     assert!(jr.step(object_file, Direction::Backward)?);
//     let rt1b = jr.get_realtime_usec(object_file).expect("head's realtime");
//     assert_eq!(rt1a, rt1b);

//     assert!(!jr.step(object_file, Direction::Backward).unwrap());
//     let rt1c = jr.get_realtime_usec(object_file).expect("head's realtime");
//     assert_eq!(rt1a, rt1c);

//     while jr.step(object_file, Direction::Forward).unwrap() {}
//     while jr.step(object_file, Direction::Backward).unwrap() {}

//     let rt1d = jr.get_realtime_usec(object_file).expect("head's realtime");
//     assert_eq!(rt1a, rt1d);

//     Ok(())
// }

// fn test_tail<M: MemoryMap>(object_file: &ObjectFile<M>) -> Result<()> {
//     let mut jr = JournalReader::default();

//     jr.set_location(object_file, Location::Tail);
//     assert!(!jr.step(object_file, Direction::Forward)?);

//     match jr.get_realtime_usec(object_file).expect_err("unset cursor") {
//         JournalError::UnsetCursor => {}
//         _ => {
//             panic!("unexpected journal error");
//         }
//     };

//     assert!(jr.step(object_file, Direction::Backward)?);
//     let rt1a = jr.get_realtime_usec(object_file).expect("tails's realtime");
//     assert_eq!(rt1a, object_file.journal_header().tail_entry_realtime);

//     assert!(jr.step(object_file, Direction::Backward)?);
//     let rt2 = jr
//         .get_realtime_usec(object_file)
//         .expect("realtime before tail");
//     assert_ne!(rt2, rt1a);

//     assert!(jr.step(object_file, Direction::Forward)?);
//     let rt1b = jr.get_realtime_usec(object_file).expect("tails's realtime");
//     assert_eq!(rt1a, rt1b);

//     assert!(!jr.step(object_file, Direction::Forward).unwrap());
//     let rt1c = jr.get_realtime_usec(object_file).expect("tails's realtime");
//     assert_eq!(rt1a, rt1c);

//     while jr.step(object_file, Direction::Backward).unwrap() {}
//     while jr.step(object_file, Direction::Forward).unwrap() {}

//     let rt1d = jr.get_realtime_usec(object_file).expect("tail's realtime");
//     assert_eq!(rt1a, rt1d);

//     Ok(())
// }

// fn test_midpoint_entry<M: MemoryMap>(object_file: &ObjectFile<M>) -> Result<()> {
//     let header = object_file.journal_header();
//     let total_entries = header.n_entries;

//     let midpoint_idx = total_entries / 2;

//     let mut jr = JournalReader::default();

//     for _ in 0..midpoint_idx {
//         assert!(jr
//             .step(object_file, Direction::Forward)
//             .expect("step to succeed"));
//     }

//     let midpoint_entry_offset = jr.get_entry_offset().expect("A valid entry offset");
//     let midpoint_entry_realtime = jr.get_realtime_usec(object_file)?;

//     jr.set_location(object_file, Location::Head);
//     assert!(jr.step(object_file, Direction::Forward).expect("no error"));
//     assert_eq!(
//         jr.get_realtime_usec(object_file)
//             .expect("realtime of head entry"),
//         object_file.journal_header().head_entry_realtime,
//     );

//     // By offset
//     {
//         jr.set_location(object_file, Location::Entry(midpoint_entry_offset));
//         assert!(jr.step(object_file, Direction::Forward).expect("no error"));

//         let entry_offset = jr.get_entry_offset().expect("A valid entry offset");
//         assert_eq!(entry_offset, midpoint_entry_offset);
//         let entry_realtime = jr.get_realtime_usec(object_file)?;
//         assert_eq!(entry_realtime, midpoint_entry_realtime);

//         jr.set_location(object_file, Location::Entry(midpoint_entry_offset));
//         assert!(jr.step(object_file, Direction::Backward).expect("no error"));

//         let entry_offset = jr.get_entry_offset().expect("A valid entry offset");
//         assert!(entry_offset < midpoint_entry_offset);
//         let entry_realtime = jr.get_realtime_usec(object_file)?;
//         assert!(entry_realtime < midpoint_entry_realtime);

//         assert!(jr.step(object_file, Direction::Forward).expect("no error"));

//         let entry_offset = jr.get_entry_offset().expect("A valid entry offset");
//         assert_eq!(entry_offset, midpoint_entry_offset);
//         let entry_realtime = jr.get_realtime_usec(object_file)?;
//         assert_eq!(entry_realtime, midpoint_entry_realtime);
//     }

//     // By realtime
//     {
//         jr.set_location(object_file, Location::Realtime(midpoint_entry_realtime));
//         assert!(jr.step(object_file, Direction::Forward).expect("no error"));

//         let entry_offset = jr.get_entry_offset().expect("A valid entry offset");
//         assert_eq!(entry_offset, midpoint_entry_offset);
//         let entry_realtime = jr.get_realtime_usec(object_file)?;
//         assert_eq!(entry_realtime, midpoint_entry_realtime);

//         jr.set_location(object_file, Location::Realtime(midpoint_entry_realtime));
//         assert!(jr.step(object_file, Direction::Backward).expect("no error"));

//         let entry_offset = jr.get_entry_offset().expect("A valid entry offset");
//         assert!(entry_offset < midpoint_entry_offset);
//         let entry_realtime = jr.get_realtime_usec(object_file)?;
//         assert!(entry_realtime < midpoint_entry_realtime);

//         assert!(jr.step(object_file, Direction::Forward).expect("no error"));

//         let entry_offset = jr.get_entry_offset().expect("A valid entry offset");
//         assert_eq!(entry_offset, midpoint_entry_offset);
//         let entry_realtime = jr.get_realtime_usec(object_file)?;
//         assert_eq!(entry_realtime, midpoint_entry_realtime);
//     }

//     Ok(())
// }

// fn test_filter<M: MemoryMap>(object_file: &ObjectFile<M>) -> Result<()> {
//     let mut jr = JournalReader::default();

//     for i in 1..4 {
//         let mut entry_offsets = Vec::new();

//         let key = format!("SVD_{i}");
//         let value = format!("svd-{i}");
//         let data = format!("{key}={value}");
//         jr.add_match(data.as_bytes());

//         jr.set_location(object_file, Location::Head);
//         while jr.step(object_file, Direction::Forward).unwrap() {
//             entry_offsets.push(jr.get_entry_offset().unwrap());
//             let ed = EntryData::from_offset(object_file, *entry_offsets.last().unwrap()).unwrap();
//             assert_eq!(ed.get_field(key.as_str()), Some(value.as_str()));
//         }

//         assert_eq!(entry_offsets.len(), 20);
//         assert!(entry_offsets.is_sorted());
//     }

//     for i in 1..4 {
//         let mut entry_offsets = Vec::new();

//         let key = format!("SVD_{i}");
//         let value = format!("svd-{i}");
//         let data = format!("{key}={value}");
//         jr.add_match(data.as_bytes());

//         jr.set_location(object_file, Location::Tail);
//         while jr.step(object_file, Direction::Backward).unwrap() {
//             entry_offsets.push(jr.get_entry_offset().unwrap());
//             let ed = EntryData::from_offset(object_file, *entry_offsets.last().unwrap()).unwrap();
//             assert_eq!(ed.get_field(key.as_str()), Some(value.as_str()));
//         }

//         assert_eq!(entry_offsets.len(), 20);
//         assert!(entry_offsets.is_sorted_by(|a, b| a > b));
//     }

//     {
//         jr.add_match(b"SVD_1=svd-1");

//         jr.set_location(object_file, Location::Tail);
//         assert!(!jr.step(object_file, Direction::Forward).unwrap());

//         jr.set_location(object_file, Location::Head);
//         assert!(!jr.step(object_file, Direction::Backward).unwrap());
//     }

//     {
//         let mut entry_offsets = Vec::new();

//         jr.add_match(b"SVD_1=svd-1");
//         jr.add_match(b"SVD_1=svd-a");

//         let mut num_svd_1 = 0;
//         let mut num_svd_a = 0;

//         jr.set_location(Location::Head);
//         while jr.step(object_file, Direction::Forward).unwrap() {
//             entry_offsets.push(jr.get_entry_offset().unwrap());
//             let ed = EntryData::from_offset(object_file, *entry_offsets.last().unwrap()).unwrap();

//             if ed.get_field("SVD_1") == Some("svd-1") {
//                 num_svd_1 += 1;
//             } else if ed.get_field("SVD_1") == Some("svd-a") {
//                 num_svd_a += 1;
//             } else {
//                 panic!("Unexpected value");
//             }
//         }

//         assert_eq!(num_svd_1, 20);
//         assert_eq!(num_svd_a, 5);
//         assert_eq!(entry_offsets.len(), 20 + 5);
//         assert!(entry_offsets.is_sorted());
//     }

//     {
//         let mut entry_offsets = Vec::new();

//         jr.add_match(b"SVD_1=svd-1");
//         jr.add_match(b"SVD_1=svd-a");

//         let mut num_svd_1 = 0;
//         let mut num_svd_a = 0;

//         jr.set_location(Location::Tail);
//         while jr.step(object_file, Direction::Backward).unwrap() {
//             entry_offsets.push(jr.get_entry_offset().unwrap());
//             let ed = EntryData::from_offset(object_file, *entry_offsets.last().unwrap()).unwrap();

//             if ed.get_field("SVD_1") == Some("svd-1") {
//                 num_svd_1 += 1;
//             } else if ed.get_field("SVD_1") == Some("svd-a") {
//                 num_svd_a += 1;
//             } else {
//                 panic!("Unexpected value");
//             }
//         }

//         assert_eq!(num_svd_1, 20);
//         assert_eq!(num_svd_a, 5);
//         assert_eq!(entry_offsets.len(), 20 + 5);
//         assert!(entry_offsets.is_sorted_by(|a, b| a > b));
//     }

//     {
//         // get realtime of first entry with SVD_1=svd-1
//         jr.add_match(b"SVD_1=svd-1");
//         jr.set_location(Location::Head);
//         assert!(jr.step(object_file, Direction::Forward).unwrap());

//         let entry_offset = jr.get_entry_offset().unwrap();
//         let ed = EntryData::from_offset(object_file, entry_offset).unwrap();
//         assert_eq!(ed.get_field("SVD_1"), Some("svd-1"));
//         let svd_1_rt = jr.get_realtime_usec(object_file).unwrap();

//         // flush matches and make sure we end up at head
//         jr.flush_matches();
//         assert!(jr.step(object_file, Direction::Forward).unwrap());
//         assert_eq!(
//             jr.get_realtime_usec(object_file).unwrap(),
//             object_file.journal_header().head_entry_realtime
//         );

//         // seek by realtime
//         for _ in 0..5 {
//             jr.add_match(b"SVD_1=svd-1");
//             jr.set_location(Location::Realtime(svd_1_rt));
//             assert!(jr.step(object_file, Direction::Forward).unwrap());
//             assert_eq!(jr.get_realtime_usec(object_file).unwrap(), svd_1_rt);

//             assert!(jr.step(object_file, Direction::Forward).unwrap());
//             assert!(jr.get_realtime_usec(object_file).unwrap() > svd_1_rt);

//             assert!(jr.step(object_file, Direction::Backward).unwrap());
//             assert!(jr.get_realtime_usec(object_file).unwrap() == svd_1_rt);

//             assert!(!jr.step(object_file, Direction::Backward).unwrap());
//         }
//     }

//     Ok(())
// }

// fn test_cursor<M: MemoryMap>(object_file: &ObjectFile<M>) -> Result<()> {
//     test_head(object_file)?;
//     test_tail(object_file)?;
//     test_midpoint_entry(object_file)?;

//     test_filter(object_file)?;

//     println!("All good!");
//     Ok(())
// }

// fn test_filter_expr<M: MemoryMap>(object_file: &ObjectFile<M>) -> Result<()> {
//     {
//         println!("Journal reader:");
//         let mut jr = JournalReader::default();

//         jr.add_match(b"SVD_1=svd-1");
//         jr.add_conjunction(object_file)?;
//         jr.add_match(b"SVD_2=svd-2");

//         jr.set_location(Location::Tail);
//         while jr.step(object_file, Direction::Backward).unwrap() {
//             let entry_offset = jr.get_entry_offset()?;
//             let entry_data = EntryData::from_offset(object_file, entry_offset)?;
//             println!("\ted[{}] = {:?}", entry_offset, entry_data);
//         }
//     }

//     let jr_lookups = object_file.stats().direct_lookups;

//     {
//         println!("Journal filter:");
//         let mut jf = journal_filter::JournalFilter::default();
//         jf.add_match(b"SVD_1=svd-1");
//         jf.set_operation(object_file, journal_filter::LogicalOp::Conjunction)?;
//         jf.add_match(b"SVD_2=svd-2");

//         let mut fe = jf.build(object_file)?;
//         fe.tail(object_file)?;
//         let mut needle_offset = u64::MAX;

//         while let Some(entry_offset) = fe.previous(object_file, needle_offset)? {
//             let entry_data = EntryData::from_offset(object_file, entry_offset)?;
//             println!("\ted[{}] = {:?}", entry_offset, entry_data);
//             needle_offset = entry_offset - 1;
//         }

//         println!("Journal filter (forwards):");
//         needle_offset += 1;
//         println!("needle_offset={:#x?}", needle_offset);
//         while let Some(entry_offset) = fe.next(object_file, needle_offset)? {
//             let entry_data = EntryData::from_offset(object_file, entry_offset)?;
//             println!("\ted[{}] = {:?}", entry_offset, entry_data);
//             needle_offset = entry_offset + 1;
//         }
//     }

//     let jf_lookups = object_file.stats().direct_lookups - jr_lookups;
//     println!("lookups: jr={}, jf={}", jr_lookups, jf_lookups);

//     Ok(())
// }

use systemd::journal;

struct JournalWrapper<'a> {
    j: journal::Journal,

    jr: JournalReader<'a, Mmap>,
}

impl<'a> JournalWrapper<'a> {
    pub fn open(path: &str) -> Result<Self> {
        let opts = journal::OpenFilesOptions::default();
        let j = opts.open_files([path])?;
        let jr = JournalReader::default();

        Ok(Self { j, jr })
    }

    pub fn match_add(&mut self, data: &str) {
        self.j.match_add(data).unwrap();
        self.jr.add_match(data.as_bytes());
    }

    pub fn match_and(&mut self, object_file: &'a ObjectFile<Mmap>) {
        self.j.match_and().unwrap();
        self.jr.add_conjunction(object_file).unwrap();
    }

    pub fn match_or(&mut self, object_file: &'a ObjectFile<Mmap>) {
        self.j.match_or().unwrap();
        self.jr.add_disjunction(object_file).unwrap();
    }

    pub fn match_flush(&mut self) {
        self.j.match_flush().unwrap();
        self.jr.flush_matches();
    }

    pub fn seek_head(&mut self) {
        self.j.seek_head().unwrap();
        self.jr.set_location(Location::Head);
    }

    pub fn seek_tail(&mut self) {
        self.j.seek_tail().unwrap();
        self.jr.set_location(Location::Tail);
    }

    pub fn seek_realtime(&mut self, usec: u64) {
        self.j.seek_realtime_usec(usec).unwrap();
        self.jr.set_location(Location::Realtime(usec));
    }

    pub fn next(&mut self, object_file: &'a ObjectFile<Mmap>) -> bool {
        let r1 = self.j.next().unwrap();
        let r2 = self.jr.step(object_file, Direction::Forward).unwrap();

        if r1 > 0 {
            if r2 {
                return r2;
            } else {
                panic!("r1: {:?}, r2: {:?}", r1, r2);
            }
        } else if r1 == 0 {
            if !r2 {
                return r2;
            } else {
                panic!("r1: {:?}, r2: {:?}", r1, r2);
            }
        } else {
            println!("WTF?");
        }

        r2
    }

    pub fn previous(&mut self, object_file: &'a ObjectFile<Mmap>) -> bool {
        let r1 = self.j.previous().unwrap();
        let r2 = self.jr.step(object_file, Direction::Backward).unwrap();

        if r1 > 0 {
            if r2 {
                return r2;
            } else {
                panic!("r1: {:?}, r2: {:?}", r1, r2);
            }
        } else if r1 == 0 {
            if !r2 {
                return r2;
            } else {
                panic!("r1: {:?}, r2: {:?}", r1, r2);
            }
        } else {
            println!("WTF?");
        }

        r2
    }

    pub fn get_realtime_usec(&mut self, object_file: &'a ObjectFile<Mmap>) -> u64 {
        let usec1 = self.j.timestamp().unwrap();
        let usec2 = self.jr.get_realtime_usec(object_file).unwrap();

        assert_eq!(usec1, usec2);
        usec1
    }
}

fn get_terms(path: &str) -> HashMap<String, Vec<String>> {
    let window_size = 8 * 1024 * 1024;
    let object_file = ObjectFile::<Mmap>::open(path, window_size).unwrap();

    let mut terms = HashMap::new();
    let mut fields = Vec::new();
    for field in object_file.fields() {
        let field = field.unwrap();
        let field = String::from(String::from_utf8_lossy(field.get_payload()).clone());
        fields.push(field.clone());
        terms.insert(field, Vec::new());
    }

    for field in fields {
        for data in object_file.field_data_objects(field.as_bytes()).unwrap() {
            let data = data.unwrap();
            if data.is_compressed() {
                continue;
            }

            let data_payload = String::from(String::from_utf8_lossy(data.get_payload()).clone());

            if data_payload.len() > 200 {
                continue;
            }

            terms.get_mut(&field).unwrap().push(data_payload);
        }
    }

    terms.retain(|_, value| !value.is_empty());
    terms
}

#[derive(Debug)]
enum SeekType {
    Head,
    Tail,
    Realtime(u64),
}

fn get_timings(path: &str) -> Vec<u64> {
    let window_size = 8 * 1024 * 1024;
    let object_file = ObjectFile::<Mmap>::open(path, window_size).unwrap();
    let mut jw: JournalWrapper<'_> = JournalWrapper::open(path).unwrap();

    let mut v = Vec::new();

    jw.seek_head();
    loop {
        if !jw.next(&object_file) {
            break;
        }

        let usec = jw.get_realtime_usec(&object_file);
        v.push(usec);
    }
    assert!(v.is_sorted());

    v
}

use rand::{prelude::*, Rng};

#[derive(Debug, Copy, Clone)]
enum SeekOperation {
    Head,
    Tail,
    Realtime(u64),
}

fn select_seek_operation(rng: &mut ThreadRng, timings: &[u64]) -> SeekOperation {
    let duplicate_timestamps = [
        1747729025279631,
        1747729025280143,
        1747729025280247,
        1747729025358451,
        1747729025387355,
        1747729025387415,
    ];

    match rng.random_range(0..3) {
        0 | 2 => SeekOperation::Head,
        1 => SeekOperation::Tail,
        20 => {
            let rt_idx = rng.random_range(0..timings.len());

            let usec = timings[rt_idx];
            if duplicate_timestamps.contains(&usec) {
                return SeekOperation::Head;
            }

            SeekOperation::Realtime(timings[rt_idx])
        }
        _ => unreachable!(),
    }
}

#[derive(Debug)]
enum MatchOr {
    None,
    One(String),
    Two(String, String),
}

fn select_match_or(rng: &mut ThreadRng, terms: &HashMap<String, Vec<String>>) -> MatchOr {
    match rng.random_range(0..3) {
        0 => MatchOr::None,
        1 => {
            let key_index = rng.random_range(0..terms.len());
            let key = terms.keys().nth(key_index).unwrap();

            let value = terms.get(key).unwrap();
            let value_index = rng.random_range(0..value.len());

            MatchOr::One(value[value_index].clone())
        }
        2 => {
            let first_term = {
                let key_index = rng.random_range(0..terms.len());
                let key = terms.keys().nth(key_index).unwrap();

                let value = terms.get(key).unwrap();
                let value_index = rng.random_range(0..value.len());

                value[value_index].clone()
            };

            let second_term = {
                let key_index = rng.random_range(0..terms.len());
                let key = terms.keys().nth(key_index).unwrap();

                let value = terms.get(key).unwrap();
                let value_index = rng.random_range(0..value.len());

                value[value_index].clone()
            };

            MatchOr::Two(first_term, second_term)
        }
        _ => {
            unreachable!()
        }
    }
}

#[derive(Debug, Clone)]
enum MatchExpr {
    None,
    OrOne(String),
    OrTwo(String, String),
    And1(String, String),
    And2(String, (String, String)),
    And3((String, String), String),
    And4((String, String), (String, String)),
}

fn select_match_expression(rng: &mut ThreadRng, terms: &HashMap<String, Vec<String>>) -> MatchExpr {
    let mor1 = select_match_or(rng, terms);
    let mor2 = select_match_or(rng, terms);

    let expr = match (mor1, mor2) {
        (MatchOr::None, MatchOr::None) => MatchExpr::None,

        (MatchOr::None, MatchOr::One(d1)) => MatchExpr::OrOne(d1),
        (MatchOr::One(d1), MatchOr::None) => MatchExpr::OrOne(d1),

        (MatchOr::None, MatchOr::Two(d1, d2)) => MatchExpr::OrTwo(d1, d2),
        (MatchOr::Two(d1, d2), MatchOr::None) => MatchExpr::OrTwo(d1, d2),

        (MatchOr::One(d1), MatchOr::One(d2)) => MatchExpr::And1(d1, d2),

        (MatchOr::One(d1), MatchOr::Two(d2, d3)) => MatchExpr::And2(d1, (d2, d3)),
        (MatchOr::Two(d1, d2), MatchOr::One(d3)) => MatchExpr::And3((d1, d2), d3),

        (MatchOr::Two(d1, d2), MatchOr::Two(d3, d4)) => MatchExpr::And4((d1, d2), (d3, d4)),
    };

    match expr.clone() {
        MatchExpr::None | MatchExpr::OrOne(_) => expr,
        MatchExpr::OrTwo(d1, d2) => {
            if d1 == d2 {
                MatchExpr::None
            } else {
                expr
            }
        }
        MatchExpr::And1(d1, d2) => {
            if d1 == d2 {
                MatchExpr::None
            } else {
                expr
            }
        }
        MatchExpr::And2(d1, (d2, d3)) => {
            if d1 == d2 || d1 == d3 || d2 == d3 {
                MatchExpr::None
            } else {
                expr
            }
        }
        MatchExpr::And3((d1, d2), d3) => {
            if d1 == d2 || d1 == d3 || d2 == d3 {
                MatchExpr::None
            } else {
                expr
            }
        }
        MatchExpr::And4((d1, d2), (d3, d4)) => {
            if d1 == d2 || d1 == d3 || d1 == d4 || d2 == d3 || d2 == d4 || d3 == d4 {
                MatchExpr::None
            } else {
                expr
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum IterationOperation {
    Next,
    Previous,
}

fn select_iteration_operation(rng: &mut ThreadRng) -> IterationOperation {
    match rng.random_range(0..2) {
        0 => IterationOperation::Next,
        1 => IterationOperation::Previous,
        _ => unreachable!(),
    }
}

fn apply_seek_operation(seek_operation: SeekOperation, jw: &mut JournalWrapper) {
    match seek_operation {
        SeekOperation::Head => jw.seek_head(),
        SeekOperation::Tail => jw.seek_tail(),
        SeekOperation::Realtime(usec) => jw.seek_realtime(usec),
    }
}

fn apply_iteration_operation<'a>(
    iteration_operation: IterationOperation,
    jw: &mut JournalWrapper<'a>,
    object_file: &'a ObjectFile<Mmap>,
) -> bool {
    match iteration_operation {
        IterationOperation::Next => jw.next(object_file),
        IterationOperation::Previous => jw.previous(object_file),
    }
}

fn apply_match_expression(match_expr: MatchExpr, jw: &mut JournalWrapper) -> bool {
    jw.match_flush();

    match match_expr.clone() {
        MatchExpr::None => {}
        MatchExpr::OrOne(d) => {
            println!("match_expr: {:?}", match_expr);
            jw.match_add(&d);
        }
        MatchExpr::OrTwo(d1, d2) => {
            println!("match_expr: {:?}", match_expr);
            jw.match_add(&d1);
            jw.match_add(&d2);
            return true;
        }
        _ => {}
    };

    return false;
}

fn unfiltered_test() {
    let path = "/tmp/user-1000.journal";
    let window_size = 8 * 1024 * 1024;
    let object_file = ObjectFile::<Mmap>::open(path, window_size).unwrap();
    let mut jw = JournalWrapper::open(path).unwrap();

    let timings = get_timings(path);

    let mut rng = rand::rng();

    let mut counter = 0;
    loop {
        let seek_operation = select_seek_operation(&mut rng, &timings);
        // println!("seek: {:?}", seek_operation);
        apply_seek_operation(seek_operation, &mut jw);

        for _ in 0..rng.random_range(0..2 * timings.len()) {
            let iteration_operation = select_iteration_operation(&mut rng);
            let found = apply_iteration_operation(iteration_operation, &mut jw, &object_file);

            if found {
                jw.get_realtime_usec(&object_file);
            }

            if counter % 1000 == 0 {
                println!("counter = {}", counter);
            }

            counter += 1;
        }
    }
}

fn filtered_test() {
    let path = "/tmp/user-1000.journal";
    let window_size = 8 * 1024 * 1024;
    let object_file = ObjectFile::<Mmap>::open(path, window_size).unwrap();
    let mut jw = JournalWrapper::open(path).unwrap();

    let terms = get_terms(path);
    let timings = get_timings(path);

    let mut rng = rand::rng();

    let mut counter = 0;
    loop {
        let match_expr = select_match_expression(&mut rng, &terms);
        let applied = apply_match_expression(match_expr.clone(), &mut jw);
        if !applied {
            continue;
        }

        let seek_operation = select_seek_operation(&mut rng, &timings);
        println!("seek: {:?}", seek_operation);
        apply_seek_operation(seek_operation, &mut jw);

        let mut num_matches = 0;
        for _ in 0..rng.random_range(0..2 * timings.len()) {
            let iteration_operation = select_iteration_operation(&mut rng);
            let found = apply_iteration_operation(iteration_operation, &mut jw, &object_file);

            if found {
                jw.get_realtime_usec(&object_file);
                num_matches += 1;
            }

            if counter % 1000 == 0 {
                println!("counter = {}", counter);
            }

            counter += 1;
        }

        println!("\tNum matches: {:?}\n", num_matches);
    }
}

fn test_case() {
    let path = "/tmp/user-1000.journal";

    let window_size = 8 * 1024 * 1024;
    let object_file = ObjectFile::<Mmap>::open(path, window_size).unwrap();
    let mut jw = JournalWrapper::open(path).unwrap();

    // let timings = get_timings(path);
    // let p = timings.iter().position(|x| *x == 1747729025444423).unwrap();
    // println!("p[{}]={}", p - 1, timings[p - 1]);
    // println!("p[{}]={}", p, timings[p]);
    // println!("p[{}]={}", p + 1, timings[p + 1]);

    println!("Seeking realtime....");
    jw.seek_realtime(1747729025358451);
    if jw.previous(&object_file) {
        let value = jw.get_realtime_usec(&object_file);
        println!("first value: {:?}", value);

        if jw.previous(&object_file) {
            let value = jw.get_realtime_usec(&object_file);
            println!("first value: {:?}", value);
        }
    }
    return;

    jw.next(&object_file);
    let value = jw.get_realtime_usec(&object_file);
    println!("second value: {:?}", value);
}

fn main() {
    // unfiltered_test();
    filtered_test();
    // test_case()

    //     altime();

    //     let args: Vec<String> = std::env::args().collect();
    //     if args.len() != 2 {
    //         eprintln!("Usage: {} <journal_file_path>", args[0]);
    //         std::process::exit(1);
    //     }

    //     if false {
    //         create_logs();
    //         return;
    //     }

    //     const WINDOW_SIZE: u64 = 4096;
    //     match ObjectFile::<Mmap>::open(&args[1], WINDOW_SIZE) {
    //         Ok(object_file) => {
    //             if true {
    //                 if let Err(e) = test_cursor(&object_file) {
    //                     panic!("Cursor tests failed: {:?}", e);
    //                 }
    //             }

    //             if true {
    //                 if let Err(e) = test_filter_expr(&object_file) {
    //                     panic!("Filter expression tests failed: {:?}", e);
    //                 }

    //                 println!("Overall stat: {:?}", object_file.stats());
    //             }
    //         }
    //         Err(e) => panic!("Failed to open journal file: {:?}", e),
    //     }
}
