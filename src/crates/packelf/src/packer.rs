//! Main packing/unpacking logic

use crate::elf::{self, ElfHeader};
use crate::stub::{self, StubParameters};
use anyhow::{bail, Context, Result};
use memmap2::Mmap;
use packelf_runtime::{calculate_checksum, PackElfHeader};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Pack (compress) an executable file
pub fn pack_file(input_path: &Path, output_path: &Path) -> Result<()> {
    // Validate input is an ELF file
    elf::validate_elf(input_path).context("Input file validation failed")?;

    // Read the entire input file
    let input_file = File::open(input_path)?;
    let input_mmap = unsafe { Mmap::map(&input_file)? };
    let input_data = &input_mmap[..];

    println!(
        "  Input size: {} bytes",
        input_data.len()
    );

    // Parse ELF header to get entry point
    let mut input_file_seek = File::open(input_path)?;
    let elf_header = ElfHeader::from_file(&mut input_file_seek)?;
    println!("  Original entry point: 0x{:x}", elf_header.entry);

    // Compress the data
    println!("  Compressing with LZ4...");
    let compressed = lz4_flex::compress(input_data);
    let compression_ratio = (compressed.len() as f64 / input_data.len() as f64) * 100.0;

    println!(
        "  Compressed size: {} bytes ({:.1}% of original)",
        compressed.len(),
        compression_ratio
    );

    // Calculate checksum
    let checksum = calculate_checksum(input_data);
    println!("  Checksum: 0x{:08x}", checksum);

    // For simplicity, we'll use a different approach:
    // Create a new ELF that decompresses and executes the original
    //
    // Structure:
    // [Stub executable] [Compressed data] [PackELF Header]

    // Generate stub
    let stub_params = StubParameters {
        compressed_offset: 0, // Will be updated
        compressed_size: compressed.len() as u64,
        uncompressed_size: input_data.len() as u64,
        checksum,
        original_entry: elf_header.entry,
    };

    let stub = stub::create_stub(stub_params)?;
    let stub_size = stub.len();

    // Calculate offset where compressed data will be
    let compressed_offset = stub_size as u64;

    // Create PackELF header
    let pack_header = PackElfHeader::new(
        compressed.len() as u64,
        input_data.len() as u64,
        checksum,
        compressed_offset,
    );

    println!("  Creating packed executable...");

    // Write output file
    let mut output = File::create(output_path)?;

    // Write stub
    output.write_all(&stub)?;

    // Write compressed data
    output.write_all(&compressed)?;

    // Write header
    output.write_all(&pack_header.to_bytes())?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = output.metadata()?.permissions();
        perms.set_mode(0o755);
        output.set_permissions(perms)?;
    }

    println!("  Done!");

    Ok(())
}

/// Unpack (decompress) a packed executable
pub fn unpack_file(input_path: &Path, output_path: &Path) -> Result<()> {
    let mut input_file = File::open(input_path)?;

    // Read the header from the end of the file
    let file_size = input_file.metadata()?.len();
    let header_offset = file_size
        .checked_sub(PackElfHeader::SIZE as u64)
        .context("File too small to contain header")?;

    input_file.seek(SeekFrom::Start(header_offset))?;
    let mut header_bytes = vec![0u8; PackElfHeader::SIZE];
    input_file.read_exact(&mut header_bytes)?;

    let header = PackElfHeader::from_bytes(&header_bytes)?;
    header.validate()?;

    println!(
        "  Compressed: {} bytes, Uncompressed: {} bytes",
        header.compressed_size, header.uncompressed_size
    );

    // Read compressed data
    input_file.seek(SeekFrom::Start(header.compressed_offset))?;
    let mut compressed = vec![0u8; header.compressed_size as usize];
    input_file.read_exact(&mut compressed)?;

    // Decompress
    println!("  Decompressing...");
    let decompressed = packelf_runtime::decompress_and_verify(
        &compressed,
        header.uncompressed_size as usize,
        header.checksum,
    )?;

    // Write output
    let mut output = File::create(output_path)?;
    output.write_all(&decompressed)?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = output.metadata()?.permissions();
        perms.set_mode(0o755);
        output.set_permissions(perms)?;
    }

    println!("  Done!");

    Ok(())
}

/// Check if a file is packed with PackELF
pub fn is_packed(path: &Path) -> Result<bool> {
    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();

    if file_size < PackElfHeader::SIZE as u64 {
        return Ok(false);
    }

    // Try to read header from end
    let header_offset = file_size - PackElfHeader::SIZE as u64;
    file.seek(SeekFrom::Start(header_offset))?;

    let mut header_bytes = vec![0u8; PackElfHeader::SIZE];
    if file.read_exact(&mut header_bytes).is_err() {
        return Ok(false);
    }

    // Try to parse header
    match PackElfHeader::from_bytes(&header_bytes) {
        Ok(header) => Ok(header.validate().is_ok()),
        Err(_) => Ok(false),
    }
}

/// Show information about a packed file
pub fn show_info(path: &Path) -> Result<()> {
    if !is_packed(path)? {
        bail!("File is not packed with PackELF");
    }

    let mut file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let header_offset = file_size - PackElfHeader::SIZE as u64;

    file.seek(SeekFrom::Start(header_offset))?;
    let mut header_bytes = vec![0u8; PackElfHeader::SIZE];
    file.read_exact(&mut header_bytes)?;

    let header = PackElfHeader::from_bytes(&header_bytes)?;

    println!("PackELF Information:");
    println!("  Format version: {}", header.version);
    println!("  Compressed size: {} bytes", header.compressed_size);
    println!("  Uncompressed size: {} bytes", header.uncompressed_size);
    println!(
        "  Compression ratio: {:.1}%",
        (header.compressed_size as f64 / header.uncompressed_size as f64) * 100.0
    );
    println!("  Checksum: 0x{:08x}", header.checksum);
    println!("  Compressed data offset: 0x{:x}", header.compressed_offset);
    println!("  Stub size: {} bytes", header.compressed_offset);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_is_packed_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty");
        File::create(&path).unwrap();

        assert!(!is_packed(&path).unwrap());
    }

    #[test]
    fn test_is_packed_with_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test");

        let header = PackElfHeader::new(100, 200, 0x12345678, 0);
        let mut file = File::create(&path).unwrap();
        file.write_all(&[0u8; 100]).unwrap(); // Some data
        file.write_all(&header.to_bytes()).unwrap();

        assert!(is_packed(&path).unwrap());
    }
}
