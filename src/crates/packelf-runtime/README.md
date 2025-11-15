# PackELF Runtime

Runtime library for PackELF executable compression. This crate provides the core compression and decompression functionality used by the `packelf` binary tool.

## Features

- Pure Rust LZ4 compression/decompression using `lz4_flex`
- Simple checksum verification
- PackELF header format serialization/deserialization
- No unsafe code (via lz4_flex's safe-encode/safe-decode features)

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
packelf-runtime = { path = "../packelf-runtime" }
```

### Basic Compression/Decompression

```rust
use packelf_runtime::{calculate_checksum, decompress, decompress_and_verify};

// Compress some data
let original = b"Hello, World!".repeat(100);
let compressed = lz4_flex::compress(&original);

// Decompress
let decompressed = decompress(&compressed, original.len()).unwrap();
assert_eq!(original.to_vec(), decompressed);

// Decompress with verification
let checksum = calculate_checksum(&original);
let verified = decompress_and_verify(&compressed, original.len(), checksum).unwrap();
assert_eq!(original.to_vec(), verified);
```

### Working with PackELF Headers

```rust
use packelf_runtime::{PackElfHeader, PACKELF_MAGIC, PACKELF_VERSION};

// Create a header
let header = PackElfHeader::new(
    1000,      // compressed_size
    2000,      // uncompressed_size
    0x12345678, // checksum
    4096,      // compressed_offset
);

// Serialize to bytes
let bytes = header.to_bytes();

// Deserialize from bytes
let parsed = PackElfHeader::from_bytes(&bytes).unwrap();
assert_eq!(parsed.compressed_size, 1000);
assert_eq!(parsed.uncompressed_size, 2000);

// Validate header
parsed.validate().unwrap();
```

## File Format

The PackELF header is 40 bytes with the following structure:

```
Offset | Size | Field
-------|------|------
0x00   | 8    | Magic bytes: "PACKELF\0"
0x08   | 4    | Format version (1)
0x0C   | 8    | Compressed size
0x14   | 8    | Uncompressed size
0x1C   | 4    | Checksum (XOR of 32-bit words)
0x20   | 8    | Compressed data offset
```

All multi-byte integers are stored in little-endian format.

## Checksum Algorithm

The checksum is a simple XOR of all 32-bit words in the data:

```rust
fn calculate_checksum(data: &[u8]) -> u32 {
    let mut checksum = 0u32;
    for chunk in data.chunks(4) {
        let mut bytes = [0u8; 4];
        bytes[..chunk.len()].copy_from_slice(chunk);
        checksum ^= u32::from_le_bytes(bytes);
    }
    checksum
}
```

This provides basic integrity checking while being very fast to compute.

## Error Handling

The library uses a custom `PackElfError` enum for error handling:

- `InvalidHeader` - Malformed or missing header
- `UnsupportedVersion` - Unknown format version
- `DecompressionError` - LZ4 decompression failed
- `ChecksumMismatch` - Data corruption detected
- `IoError` - I/O operation failed

All errors implement `std::error::Error` and can be used with `anyhow` or other error handling libraries.

## Performance

LZ4 is chosen for its excellent decompression speed (typically 2-4 GB/s) while still providing good compression ratios (typically 40-60% for executables).

The `lz4_flex` crate is a pure Rust implementation that performs comparably to the reference C implementation.

## Safety

This library uses only safe Rust code. The `lz4_flex` dependency is configured to use safe encoding and decoding by default.

## Testing

Run the test suite:

```bash
cargo test --package packelf-runtime
```

All tests should pass:
- `test_header_serialization` - Header round-trip
- `test_checksum` - Checksum calculation
- `test_compress_decompress` - LZ4 operations
- `test_decompress_and_verify` - Verification logic
- `test_decompress_and_verify_bad_checksum` - Error handling

## License

This is a demonstration implementation created for educational purposes.
