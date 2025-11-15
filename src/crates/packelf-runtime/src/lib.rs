//! PackELF Runtime - Decompression library for packed executables
//!
//! This library provides the runtime decompression functionality for executables
//! packed with PackELF. It includes the decompression logic that runs when a
//! packed binary is executed.

use std::error::Error;
use std::fmt;

/// Magic bytes that identify a PackELF packed binary
pub const PACKELF_MAGIC: &[u8; 8] = b"PACKELF\0";

/// Version of the PackELF format
pub const PACKELF_VERSION: u32 = 1;

/// Header stored at the end of packed binaries
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PackElfHeader {
    /// Magic bytes for identification
    pub magic: [u8; 8],
    /// Format version
    pub version: u32,
    /// Size of the compressed data
    pub compressed_size: u64,
    /// Size of the original uncompressed data
    pub uncompressed_size: u64,
    /// Checksum of uncompressed data (simple XOR for now)
    pub checksum: u32,
    /// Offset in the file where compressed data starts
    pub compressed_offset: u64,
}

impl PackElfHeader {
    /// Size of the header in bytes
    pub const SIZE: usize = std::mem::size_of::<Self>();

    /// Create a new header
    pub fn new(
        compressed_size: u64,
        uncompressed_size: u64,
        checksum: u32,
        compressed_offset: u64,
    ) -> Self {
        Self {
            magic: *PACKELF_MAGIC,
            version: PACKELF_VERSION,
            compressed_size,
            uncompressed_size,
            checksum,
            compressed_offset,
        }
    }

    /// Serialize header to bytes
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..8].copy_from_slice(&self.magic);
        bytes[8..12].copy_from_slice(&self.version.to_le_bytes());
        bytes[12..20].copy_from_slice(&self.compressed_size.to_le_bytes());
        bytes[20..28].copy_from_slice(&self.uncompressed_size.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.checksum.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.compressed_offset.to_le_bytes());
        bytes
    }

    /// Deserialize header from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PackElfError> {
        if bytes.len() < Self::SIZE {
            return Err(PackElfError::InvalidHeader("Header too short".into()));
        }

        let mut magic = [0u8; 8];
        magic.copy_from_slice(&bytes[0..8]);

        if &magic != PACKELF_MAGIC {
            return Err(PackElfError::InvalidHeader("Invalid magic bytes".into()));
        }

        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let compressed_size = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
        let uncompressed_size = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
        let checksum = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        let compressed_offset = u64::from_le_bytes(bytes[32..40].try_into().unwrap());

        if version != PACKELF_VERSION {
            return Err(PackElfError::UnsupportedVersion(version));
        }

        Ok(Self {
            magic,
            version,
            compressed_size,
            uncompressed_size,
            checksum,
            compressed_offset,
        })
    }

    /// Validate the header
    pub fn validate(&self) -> Result<(), PackElfError> {
        if &self.magic != PACKELF_MAGIC {
            return Err(PackElfError::InvalidHeader("Invalid magic bytes".into()));
        }
        if self.version != PACKELF_VERSION {
            return Err(PackElfError::UnsupportedVersion(self.version));
        }
        if self.compressed_size == 0 || self.uncompressed_size == 0 {
            return Err(PackElfError::InvalidHeader("Invalid sizes".into()));
        }
        Ok(())
    }
}

/// Errors that can occur during decompression
#[derive(Debug)]
pub enum PackElfError {
    /// Invalid header format
    InvalidHeader(String),
    /// Unsupported version
    UnsupportedVersion(u32),
    /// Decompression failed
    DecompressionError(String),
    /// Checksum mismatch
    ChecksumMismatch { expected: u32, actual: u32 },
    /// I/O error
    IoError(std::io::Error),
}

impl fmt::Display for PackElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackElfError::InvalidHeader(msg) => write!(f, "Invalid header: {}", msg),
            PackElfError::UnsupportedVersion(v) => write!(f, "Unsupported version: {}", v),
            PackElfError::DecompressionError(msg) => write!(f, "Decompression error: {}", msg),
            PackElfError::ChecksumMismatch { expected, actual } => {
                write!(f, "Checksum mismatch: expected {}, got {}", expected, actual)
            }
            PackElfError::IoError(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl Error for PackElfError {}

impl From<std::io::Error> for PackElfError {
    fn from(e: std::io::Error) -> Self {
        PackElfError::IoError(e)
    }
}

/// Calculate a simple checksum of data (XOR of all 32-bit words)
pub fn calculate_checksum(data: &[u8]) -> u32 {
    let mut checksum = 0u32;
    for chunk in data.chunks(4) {
        let mut bytes = [0u8; 4];
        bytes[..chunk.len()].copy_from_slice(chunk);
        checksum ^= u32::from_le_bytes(bytes);
    }
    checksum
}

/// Decompress data using LZ4
pub fn decompress(compressed: &[u8], uncompressed_size: usize) -> Result<Vec<u8>, PackElfError> {
    lz4_flex::decompress(compressed, uncompressed_size).map_err(|e| {
        PackElfError::DecompressionError(format!("LZ4 decompression failed: {}", e))
    })
}

/// Decompress and verify data
pub fn decompress_and_verify(
    compressed: &[u8],
    uncompressed_size: usize,
    expected_checksum: u32,
) -> Result<Vec<u8>, PackElfError> {
    let decompressed = decompress(compressed, uncompressed_size)?;

    let actual_checksum = calculate_checksum(&decompressed);
    if actual_checksum != expected_checksum {
        return Err(PackElfError::ChecksumMismatch {
            expected: expected_checksum,
            actual: actual_checksum,
        });
    }

    Ok(decompressed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_serialization() {
        let header = PackElfHeader::new(1000, 2000, 0x12345678, 4096);
        let bytes = header.to_bytes();
        let decoded = PackElfHeader::from_bytes(&bytes).unwrap();

        assert_eq!(decoded.compressed_size, header.compressed_size);
        assert_eq!(decoded.uncompressed_size, header.uncompressed_size);
        assert_eq!(decoded.checksum, header.checksum);
        assert_eq!(decoded.compressed_offset, header.compressed_offset);
    }

    #[test]
    fn test_checksum() {
        let data = b"Hello, World!";
        let checksum1 = calculate_checksum(data);
        let checksum2 = calculate_checksum(data);
        assert_eq!(checksum1, checksum2);

        let different = b"Hello, World?";
        let checksum3 = calculate_checksum(different);
        assert_ne!(checksum1, checksum3);
    }

    #[test]
    fn test_compress_decompress() {
        let original = b"Hello, World! This is a test string for compression.".repeat(100);
        let compressed = lz4_flex::compress(&original);
        let decompressed = decompress(&compressed, original.len()).unwrap();
        assert_eq!(original.to_vec(), decompressed);
    }

    #[test]
    fn test_decompress_and_verify() {
        let original = b"Test data for verification".repeat(50);
        let compressed = lz4_flex::compress(&original);
        let checksum = calculate_checksum(&original);

        let result = decompress_and_verify(&compressed, original.len(), checksum).unwrap();
        assert_eq!(original.to_vec(), result);
    }

    #[test]
    fn test_decompress_and_verify_bad_checksum() {
        let original = b"Test data";
        let compressed = lz4_flex::compress(original);
        let bad_checksum = 0xDEADBEEF;

        let result = decompress_and_verify(&compressed, original.len(), bad_checksum);
        assert!(matches!(result, Err(PackElfError::ChecksumMismatch { .. })));
    }
}
