//! Decompression stub generation
//!
//! This module generates a small executable stub that will decompress
//! the packed binary at runtime.

use anyhow::Result;

/// Generate the decompression stub as a standalone executable
///
/// The stub is a minimal program that:
/// 1. Reads its own executable file
/// 2. Finds the PackELF header at the end
/// 3. Decompresses the original executable
/// 4. Writes it to a temporary file
/// 5. Executes the temporary file with the original arguments
/// 6. Cleans up the temporary file
pub fn generate_stub() -> &'static [u8] {
    // This would ideally be a pre-compiled minimal stub
    // For simplicity, we'll embed the stub source and compile it inline
    // or use a pre-built binary blob

    // In a real implementation, you would:
    // 1. Have a separate stub project that builds to a tiny executable
    // 2. Include that binary using include_bytes!()
    // 3. Patch it with runtime parameters

    // For now, we'll use a placeholder approach where the stub is built
    // as part of the build process
    STUB_TEMPLATE
}

/// Pre-compiled stub binary
///
/// This is the native packelf-stub binary that handles in-memory decompression
/// using memfd_create + execveat when available, with fallbacks to /dev/shm and /tmp.
///
/// The stub is built separately and included at compile time.
#[cfg(all(target_os = "linux", feature = "native_stub"))]
const STUB_TEMPLATE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/packelf-stub"));

#[cfg(not(all(target_os = "linux", feature = "native_stub")))]
const STUB_TEMPLATE: &[u8] = &[];

/// Calculate the size needed for the stub
#[allow(dead_code)]
pub fn stub_size() -> usize {
    // The stub consists of:
    // 1. ELF header
    // 2. Small decompression code
    // 3. Runtime library code for LZ4 decompression
    //
    // For now, we'll estimate this. In production, this would be
    // the actual size of the compiled stub.
    8192 // 8KB should be enough for a minimal stub
}

/// Parameters that need to be patched into the stub at runtime
#[derive(Debug)]
pub struct StubParameters {
    /// Offset where compressed data starts
    pub compressed_offset: u64,
    /// Size of compressed data
    pub compressed_size: u64,
    /// Original uncompressed size
    pub uncompressed_size: u64,
    /// Checksum of uncompressed data
    pub checksum: u32,
    /// Original entry point
    pub original_entry: u64,
}

/// Patch parameters into the stub
pub fn patch_stub(stub: &mut Vec<u8>, params: &StubParameters) -> Result<()> {
    // In a real implementation, this would locate specific markers
    // in the stub binary and replace them with actual values
    //
    // For example:
    // 1. Find a magic marker like "COMPOFF\0"
    // 2. Replace the next 8 bytes with compressed_offset
    // etc.

    // For our simplified version, we'll just append the parameters
    // at the end and have the stub know to read them from a fixed location

    stub.extend_from_slice(&params.compressed_offset.to_le_bytes());
    stub.extend_from_slice(&params.compressed_size.to_le_bytes());
    stub.extend_from_slice(&params.uncompressed_size.to_le_bytes());
    stub.extend_from_slice(&params.checksum.to_le_bytes());
    stub.extend_from_slice(&params.original_entry.to_le_bytes());

    Ok(())
}

/// Create a complete stub ready to be used
pub fn create_stub(params: StubParameters) -> Result<Vec<u8>> {
    let mut stub = generate_stub().to_vec();

    if stub.is_empty() {
        // For the initial version, we'll create a simple script-based stub
        // This is a fallback for when we don't have a compiled stub yet
        stub = create_script_stub(&params)?;
    } else {
        patch_stub(&mut stub, &params)?;
    }

    Ok(stub)
}

/// Create a simple shell script stub as a fallback
///
/// This creates a self-extracting shell script that can decompress
/// the binary. It's larger than a native stub but easier to implement initially.
fn create_script_stub(params: &StubParameters) -> Result<Vec<u8>> {
    let script = format!(
        r#"#!/bin/sh
# PackELF self-extracting archive
# This is a temporary implementation using a shell script
# A native stub would be much smaller

COMPRESSED_OFFSET={}
COMPRESSED_SIZE={}
UNCOMPRESSED_SIZE={}

# Create a temporary file
TMPFILE=$(mktemp)
trap "rm -f $TMPFILE" EXIT

# Extract compressed data from this file
tail -c +$((COMPRESSED_OFFSET + 1)) "$0" | head -c $COMPRESSED_SIZE > "$TMPFILE.lz4"

# Note: This requires lz4 to be installed
# A real implementation would include the decompression code
if ! command -v lz4 >/dev/null 2>&1; then
    echo "Error: lz4 not found. This is a development stub." >&2
    exit 1
fi

# Decompress
lz4 -d "$TMPFILE.lz4" "$TMPFILE" >/dev/null 2>&1
rm -f "$TMPFILE.lz4"
chmod +x "$TMPFILE"

# Execute the decompressed binary with original arguments
exec "$TMPFILE" "$@"
"#,
        params.compressed_offset, params.compressed_size, params.uncompressed_size
    );

    Ok(script.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stub_parameters() {
        let params = StubParameters {
            compressed_offset: 0x1000,
            compressed_size: 0x5000,
            uncompressed_size: 0x10000,
            checksum: 0x12345678,
            original_entry: 0x400000,
        };

        let stub = create_stub(params).unwrap();
        assert!(!stub.is_empty());
    }

    #[test]
    fn test_stub_size() {
        assert!(stub_size() > 0);
    }
}
