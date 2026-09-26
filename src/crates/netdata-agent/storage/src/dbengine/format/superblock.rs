//! The superblocks of data files and journals (`struct rrdeng_df_sb`, `struct rrdeng_jf_sb` in
//! `rrddiskprotocol.h`): the first 4096 bytes of every `.ndf` and `.njf`. Brief `knowledge/brief-dbengine-s0.md` §2.1.

use super::BLOCK_SIZE;

/// `RRDENG_DF_MAGIC` and `RRDENG_JF_MAGIC`, in 32-byte fields.
pub const DATAFILE_MAGIC: &[u8] = b"netdata-data-file";
pub const JOURNAL_MAGIC: &[u8] = b"netdata-journal-file";
/// `RRDENG_DF_VER` and `RRDENG_JF_VER`, in 16-byte fields.
pub const VERSION: &[u8] = b"1.0";

const MAGIC_FIELD: usize = 32;
const VERSION_FIELD: usize = 16;
/// The data file superblock's `tier` byte, always 1 whatever the directory's tier.
const TIER_AT: usize = MAGIC_FIELD + VERSION_FIELD;

/// Why C refuses a superblock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperblockError {
    /// `check_file_properties()`: fewer than 4096 bytes.
    TooShort,
    Magic,
    Version,
    /// A data file whose `tier` byte is not 1.
    Tier,
}

impl std::fmt::Display for SuperblockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SuperblockError::TooShort => "shorter than a superblock",
            SuperblockError::Magic => "invalid magic number",
            SuperblockError::Version => "invalid version",
            SuperblockError::Tier => "invalid tier",
        })
    }
}

impl std::error::Error for SuperblockError {}

fn encode(magic: &[u8]) -> [u8; BLOCK_SIZE] {
    let mut b = [0u8; BLOCK_SIZE];
    b[..magic.len()].copy_from_slice(magic);
    b[MAGIC_FIELD..MAGIC_FIELD + VERSION.len()].copy_from_slice(VERSION);
    b
}

/// `create_data_file()`'s superblock: magic, version, tier 1, zeros.
pub fn encode_datafile() -> [u8; BLOCK_SIZE] {
    let mut b = encode(DATAFILE_MAGIC);
    b[TIER_AT] = 1;
    b
}

/// `create_journal_file()`'s superblock.
pub fn encode_journal() -> [u8; BLOCK_SIZE] {
    encode(JOURNAL_MAGIC)
}

/// `strncmp(field, text, field.len())`: equal up to `text`'s terminating NUL; what follows it is not compared.
fn strncmp_eq(field: &[u8], text: &[u8]) -> bool {
    field.len() > text.len() && field[..text.len()] == *text && field[text.len()] == 0
}

fn check(b: &[u8], magic: &[u8]) -> Result<(), SuperblockError> {
    if b.len() < BLOCK_SIZE {
        return Err(SuperblockError::TooShort);
    }
    if !strncmp_eq(&b[..MAGIC_FIELD], magic) {
        return Err(SuperblockError::Magic);
    }
    if !strncmp_eq(&b[MAGIC_FIELD..MAGIC_FIELD + VERSION_FIELD], VERSION) {
        return Err(SuperblockError::Version);
    }
    Ok(())
}

/// `check_data_file_superblock()`.
pub fn check_datafile(b: &[u8]) -> Result<(), SuperblockError> {
    check(b, DATAFILE_MAGIC)?;
    if b[TIER_AT] != 1 {
        return Err(SuperblockError::Tier);
    }
    Ok(())
}

/// `check_journal_file_superblock()`.
pub fn check_journal(b: &[u8]) -> Result<(), SuperblockError> {
    check(b, JOURNAL_MAGIC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superblocks_as_c_checks_them() {
        assert_eq!(check_datafile(&encode_datafile()), Ok(()));
        assert_eq!(check_journal(&encode_journal()), Ok(()));
        assert_eq!(
            check_journal(&encode_datafile()),
            Err(SuperblockError::Magic)
        );
        // the NUL after the magic is compared, what follows it is not
        let mut b = encode_datafile();
        b[17] = b'x';
        assert_eq!(check_datafile(&b), Err(SuperblockError::Magic));
        let mut b = encode_datafile();
        b[18..32].fill(0xAA);
        b[36..48].fill(0xAA);
        assert_eq!(check_datafile(&b), Ok(()));
        let mut b = encode_datafile();
        b[34] = b'1';
        assert_eq!(check_datafile(&b), Err(SuperblockError::Version));
        let mut b = encode_datafile();
        b[48] = 2;
        assert_eq!(check_datafile(&b), Err(SuperblockError::Tier));
        assert_eq!(
            check_datafile(&encode_datafile()[..4095]),
            Err(SuperblockError::TooShort)
        );
    }
}
