//! dbengine's on-disk structures (`rrddiskprotocol.h`, `journalfile.h`), their CRCs and C's validation rules
//! (brief `knowledge/brief-dbengine-s0.md` §2 in the status repository). Every integer is little-endian, as C writes
//! it on the supported targets; the layouts are C's 64-bit ones.

pub mod crc;
pub mod descriptor;
pub mod extent;
pub mod journal_v1;
pub mod page;
pub mod superblock;

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;

/// `RRDENG_BLOCK_SIZE`: the unit of superblocks, journal records and extent alignment.
pub const BLOCK_SIZE: usize = 4096;
/// `RRDENG_MAX_PAGES_PER_EXTENT`.
pub const MAX_PAGES_PER_EXTENT: usize = 109;
/// `MAX_EXTENT_UNCOMPRESSED_SIZE`: every page of a full extent at its largest (a gorilla page may take 512 bytes more).
pub const MAX_EXTENT_UNCOMPRESSED_SIZE: usize = MAX_PAGES_PER_EXTENT * (BLOCK_SIZE + 512);

/// `DATAFILE_PREFIX`, `WALFILE_PREFIX` and their extensions.
const DATAFILE_PREFIX: &str = "datafile-";
const DATAFILE_EXTENSION: &str = ".ndf";
const JOURNAL_PREFIX: &str = "journalfile-";
const JOURNAL_EXTENSION: &str = ".njf";
const JOURNAL_V2_EXTENSION: &str = ".njfv2";

/// The file kinds of a dbengine tier directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Datafile,
    Journal,
    JournalV2,
}

impl FileKind {
    fn parts(self) -> (&'static str, &'static str) {
        match self {
            FileKind::Datafile => (DATAFILE_PREFIX, DATAFILE_EXTENSION),
            FileKind::Journal => (JOURNAL_PREFIX, JOURNAL_EXTENSION),
            FileKind::JournalV2 => (JOURNAL_PREFIX, JOURNAL_V2_EXTENSION),
        }
    }
}

/// `generate_datafilepath()` / `journalfile_v1_generate_path()` / `journalfile_v2_generate_path()` without the
/// directory: `<prefix><tier>-<fileno>` with `RRDENG_FILE_NUMBER_PRINT_TMPL` (`%1.1u-%10.10u`), then the extension.
pub fn file_name(kind: FileKind, tier: u32, fileno: u32) -> String {
    let (prefix, extension) = kind.parts();
    format!("{prefix}{tier}-{fileno:010}{extension}")
}

/// The `(tier, fileno)` of a file name `file_name()` produces (`RRDENG_FILE_NUMBER_SCAN_TMPL`, `%1u-%10u`).
pub fn parse_file_name(kind: FileKind, name: &str) -> Option<(u32, u32)> {
    let (prefix, extension) = kind.parts();
    let numbers = name.strip_prefix(prefix)?.strip_suffix(extension)?;
    let (tier, fileno) = numbers.split_once('-')?;
    if tier.len() != 1 || fileno.len() != 10 {
        return None;
    }
    Some((tier.parse().ok()?, fileno.parse().ok()?))
}

/// The directory of a tier under the cache directory: `dbengine` for tier 0, `dbengine-tier<N>` above
/// (`netdata_conf_dbengine_init()`).
pub fn tier_dir_name(tier: usize) -> String {
    if tier == 0 {
        "dbengine".to_string()
    } else {
        format!("dbengine-tier{tier}")
    }
}

/// Positioned reads, from a file or from memory.
pub trait ReadAt {
    /// Fills `buf` from `offset`; an error when the source ends first.
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;
    /// The source's size in bytes.
    fn size(&self) -> io::Result<u64>;
}

impl ReadAt for File {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        FileExt::read_exact_at(self, buf, offset)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
}

impl ReadAt for [u8] {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        let end = start
            .checked_add(buf.len())
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        let src = self.get(start..end).ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(src);
        Ok(())
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_as_c_prints_them() {
        assert_eq!(
            file_name(FileKind::Datafile, 1, 3),
            "datafile-1-0000000003.ndf"
        );
        assert_eq!(
            file_name(FileKind::Journal, 1, 3),
            "journalfile-1-0000000003.njf"
        );
        assert_eq!(
            file_name(FileKind::JournalV2, 1, 42),
            "journalfile-1-0000000042.njfv2"
        );
        assert_eq!(
            parse_file_name(FileKind::JournalV2, "journalfile-1-0000000042.njfv2"),
            Some((1, 42))
        );
        assert_eq!(
            parse_file_name(FileKind::Journal, "journalfile-1-0000000042.njfv2"),
            None
        );
        assert_eq!(
            parse_file_name(FileKind::Datafile, "datafile-1-42.ndf"),
            None
        );
        assert_eq!(tier_dir_name(0), "dbengine");
        assert_eq!(tier_dir_name(2), "dbengine-tier2");
        assert_eq!(MAX_EXTENT_UNCOMPRESSED_SIZE, 502_272);
    }

    #[test]
    fn memory_reads_stop_at_the_end() {
        let data = [1u8, 2, 3, 4];
        let mut buf = [0u8; 2];
        data[..].read_exact_at(&mut buf, 2).unwrap();
        assert_eq!(buf, [3, 4]);
        assert!(data[..].read_exact_at(&mut buf, 3).is_err());
    }
}
