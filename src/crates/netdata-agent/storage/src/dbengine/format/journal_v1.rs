//! Journal v1 (`.njf`) transactions (`struct rrdeng_jf_transaction_header`, `_store_data`, `_trailer` in
//! `rrddiskprotocol.h`): one per 4096-byte block after the superblock, each describing an extent of the data file.
//! Encoding follows `rrdengine.c`, the replay loop `journalfile.c` (`journalfile_iterate_transactions()`,
//! `journalfile_replay_transaction()`, `journalfile_restore_extent_metadata()`), 1 MiB chunks included. Brief
//! `knowledge/brief-dbengine-s0.md` §2.4 in the status repository.

use std::io;

use super::crc::{crc_bytes, crc_matches, crc32};
use super::descriptor::{DESCRIPTOR_SIZE, PageDescriptor};
use super::{BLOCK_SIZE, MAX_EXTENT_UNCOMPRESSED_SIZE, MAX_PAGES_PER_EXTENT, ReadAt};

/// `STORE_PADDING`, `STORE_DATA`, `STORE_LOGS`.
pub const STORE_PADDING: u8 = 0;
pub const STORE_DATA: u8 = 1;

const HEADER_SIZE: usize = 15;
const TRAILER_SIZE: usize = 4;
const STORE_DATA_FIXED: usize = 13;
/// `READAHEAD_BYTES`: the replay reads the journal in chunks this large.
const CHUNK: u64 = 1024 * 1024;

/// A STORE_DATA payload: where the extent is and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreData {
    pub extent_offset: u64,
    /// The extent's size without its padding.
    pub extent_size: u32,
    pub descriptors: Vec<PageDescriptor>,
}

/// What the replay met, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A record header or record that does not fit where it starts ("corrupted transaction record, skipping."):
    /// parsing moves to the next block.
    Corrupt {
        pos: u64,
    },
    /// "transaction ... CRC32 check: FAILED": the record is ignored, its id still counts.
    CrcFailed {
        pos: u64,
        id: u64,
    },
    /// "unknown transaction type, skipping record."
    UnknownType {
        pos: u64,
        id: u64,
        record_type: u8,
    },
    /// "corrupted transaction payload.": too few bytes for its descriptors, or an impossible extent size.
    CorruptPayload {
        pos: u64,
        id: u64,
    },
    StoreData {
        pos: u64,
        id: u64,
        data: StoreData,
    },
}

/// The result of a replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub events: Vec<Event>,
    /// The largest transaction id seen, at least 1 (`max_trans_id`).
    pub max_id: u64,
    /// Set when a read failed; the replay stops there (C logs and keeps what it replayed).
    pub read_error: bool,
}

/// `valid_extent_disk_size()`: 47..=506315 bytes.
fn valid_extent_size(size: u32) -> bool {
    let min = 6 + DESCRIPTOR_SIZE + 4;
    let max = 6 + DESCRIPTOR_SIZE * MAX_PAGES_PER_EXTENT + MAX_EXTENT_UNCOMPRESSED_SIZE + 4;
    (min..=max).contains(&(size as usize))
}

/// A STORE_DATA transaction in its 4096-byte block.
pub fn encode_transaction(id: u64, data: &StoreData) -> [u8; BLOCK_SIZE] {
    let pages = data.descriptors.len();
    let payload_length = STORE_DATA_FIXED + DESCRIPTOR_SIZE * pages;
    let mut b = [0u8; BLOCK_SIZE];
    b[0] = STORE_DATA;
    b[5..13].copy_from_slice(&id.to_le_bytes());
    b[13..15].copy_from_slice(&(payload_length as u16).to_le_bytes());
    b[15..23].copy_from_slice(&data.extent_offset.to_le_bytes());
    b[23..27].copy_from_slice(&data.extent_size.to_le_bytes());
    b[27] = pages as u8;
    for (i, d) in data.descriptors.iter().enumerate() {
        let at = HEADER_SIZE + STORE_DATA_FIXED + i * DESCRIPTOR_SIZE;
        b[at..at + DESCRIPTOR_SIZE].copy_from_slice(&d.encode());
    }
    let end = HEADER_SIZE + payload_length;
    let crc = crc32(&b[..end]);
    b[end..end + TRAILER_SIZE].copy_from_slice(&crc_bytes(crc));
    b
}

/// `journalfile_replay_transaction()` on the bytes left in the chunk: the record's size (0 for padding or a
/// corrupt record) and the id it read.
fn one(b: &[u8], pos: u64, events: &mut Vec<Event>) -> (usize, u64) {
    if b[0] == STORE_PADDING {
        return (0, 0);
    }
    if HEADER_SIZE > b.len() {
        events.push(Event::Corrupt { pos });
        return (0, 0);
    }
    let id = u64::from_le_bytes(b[5..13].try_into().unwrap_or_default());
    let payload_length = u16::from_le_bytes([b[13], b[14]]) as usize;
    let size = HEADER_SIZE + payload_length + TRAILER_SIZE;
    if size > b.len() {
        events.push(Event::Corrupt { pos });
        return (0, id);
    }
    let end = HEADER_SIZE + payload_length;
    if !crc_matches(&b[end..end + TRAILER_SIZE], crc32(&b[..end])) {
        events.push(Event::CrcFailed { pos, id });
        return (size, id);
    }
    if b[0] != STORE_DATA {
        events.push(Event::UnknownType {
            pos,
            id,
            record_type: b[0],
        });
        return (size, id);
    }
    // journalfile_restore_extent_metadata()
    let payload = &b[HEADER_SIZE..end];
    let pages = payload.get(12).copied().unwrap_or(0) as usize;
    let extent_size = payload
        .get(8..12)
        .map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    if STORE_DATA_FIXED + DESCRIPTOR_SIZE * pages > payload_length
        || !valid_extent_size(extent_size)
    {
        events.push(Event::CorruptPayload { pos, id });
        return (size, id);
    }
    let descriptors = (0..pages)
        .filter_map(|i| PageDescriptor::decode(&payload[STORE_DATA_FIXED + i * DESCRIPTOR_SIZE..]))
        .collect();
    events.push(Event::StoreData {
        pos,
        id,
        data: StoreData {
            extent_offset: u64::from_le_bytes(payload[0..8].try_into().unwrap_or_default()),
            extent_size,
            descriptors,
        },
    });
    (size, id)
}

/// `journalfile_iterate_transactions()`: every record of a journal of `file_size` bytes (floored to whole blocks, as
/// C floors the file size), read in 1 MiB chunks; a record crossing a chunk's end is corrupt and parsing resumes at
/// the next block.
pub fn replay<R: ReadAt + ?Sized>(r: &R, file_size: u64) -> io::Result<Replay> {
    let size = file_size / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
    let mut out = Replay {
        events: Vec::new(),
        max_id: 1,
        read_error: false,
    };
    let mut pos = BLOCK_SIZE as u64;
    let mut buf = Vec::new();
    while pos < size {
        let n = (size - pos).min(CHUNK) as usize;
        buf.resize(n, 0);
        if r.read_exact_at(&mut buf, pos).is_err() {
            out.read_error = true;
            break;
        }
        let mut i = 0usize;
        while i < n {
            let (record, id) = one(&buf[i..n], pos + i as u64, &mut out.events);
            i = if record != 0 {
                i + record
            } else {
                (i + BLOCK_SIZE) / BLOCK_SIZE * BLOCK_SIZE
            };
            out.max_id = out.max_id.max(id);
        }
        pos += CHUNK;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbengine::format::descriptor::PAGE_TYPE_ARRAY_32BIT;

    fn data(pages: usize) -> StoreData {
        StoreData {
            extent_offset: 4096,
            extent_size: 4096,
            descriptors: (0..pages)
                .map(|i| PageDescriptor::array(PAGE_TYPE_ARRAY_32BIT, [i as u8; 16], 16, 1, 2))
                .collect(),
        }
    }

    fn journal(blocks: &[[u8; BLOCK_SIZE]]) -> Vec<u8> {
        let mut j = vec![0u8; BLOCK_SIZE];
        for b in blocks {
            j.extend_from_slice(b);
        }
        j
    }

    #[test]
    fn records_replay_as_c() {
        let d = data(2);
        let j = journal(&[
            encode_transaction(7, &d),
            [0u8; BLOCK_SIZE],
            encode_transaction(9, &d),
        ]);
        let r = replay(&j[..], j.len() as u64).unwrap();
        assert_eq!(
            r.events,
            [
                Event::StoreData {
                    pos: 4096,
                    id: 7,
                    data: d.clone()
                },
                Event::StoreData {
                    pos: 12288,
                    id: 9,
                    data: d
                }
            ]
        );
        assert_eq!(r.max_id, 9);
        // an empty journal still starts ids at 1; a partial last block is not read
        let r = replay(&journal(&[])[..], 4096 + 100).unwrap();
        assert_eq!((r.events.len(), r.max_id), (0, 1));
    }

    #[test]
    fn broken_records_as_c() {
        let d = data(1);
        let mut crc = encode_transaction(20, &d);
        crc[30] ^= 1;
        let mut unknown = encode_transaction(21, &d);
        unknown[0] = 2;
        let end = HEADER_SIZE + STORE_DATA_FIXED + DESCRIPTOR_SIZE;
        let c = crc32(&unknown[..end]);
        unknown[end..end + 4].copy_from_slice(&crc_bytes(c));
        let mut bad_size = encode_transaction(
            22,
            &StoreData {
                extent_size: 46,
                ..d.clone()
            },
        );
        let c = crc32(&bad_size[..end]);
        bad_size[end..end + 4].copy_from_slice(&crc_bytes(c));
        // a payload length larger than its descriptors need is accepted
        let mut longer = encode_transaction(23, &d);
        longer[13..15]
            .copy_from_slice(&((STORE_DATA_FIXED + DESCRIPTOR_SIZE + 8) as u16).to_le_bytes());
        let lend = end + 8;
        let c = crc32(&longer[..lend]);
        longer[lend..lend + 4].copy_from_slice(&crc_bytes(c));
        let j = journal(&[crc, unknown, bad_size, longer]);
        let r = replay(&j[..], j.len() as u64).unwrap();
        assert_eq!(
            r.events,
            [
                Event::CrcFailed { pos: 4096, id: 20 },
                Event::UnknownType {
                    pos: 8192,
                    id: 21,
                    record_type: 2
                },
                Event::CorruptPayload { pos: 12288, id: 22 },
                Event::StoreData {
                    pos: 16384,
                    id: 23,
                    data: d
                }
            ]
        );
        assert_eq!(r.max_id, 23);
    }

    #[test]
    fn a_record_across_a_chunk_is_corrupt() {
        // records back to back until one crosses the first 1 MiB
        let d = data(100);
        let record = encode_transaction(1, &d);
        let len = HEADER_SIZE + STORE_DATA_FIXED + DESCRIPTOR_SIZE * 100 + TRAILER_SIZE;
        let mut j = vec![0u8; BLOCK_SIZE];
        while j.len() < BLOCK_SIZE + CHUNK as usize + BLOCK_SIZE {
            j.extend_from_slice(&record[..len]);
        }
        j.resize(j.len().div_ceil(BLOCK_SIZE) * BLOCK_SIZE, 0);
        let r = replay(&j[..], j.len() as u64).unwrap();
        let corrupt = r
            .events
            .iter()
            .filter(|e| matches!(e, Event::Corrupt { .. }))
            .count();
        assert_eq!(corrupt, 1);
    }
}
