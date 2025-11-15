//! Simple ELF file parsing and manipulation
//!
//! This module provides basic ELF file handling for 64-bit Linux executables.

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// ELF magic bytes
pub const ELF_MAGIC: &[u8; 4] = b"\x7fELF";

/// ELF class (32 or 64 bit)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ElfClass {
    Elf32 = 1,
    Elf64 = 2,
}

/// Basic ELF header information we care about
#[derive(Debug, Clone)]
pub struct ElfHeader {
    /// ELF class (32 or 64 bit)
    pub class: ElfClass,
    /// Entry point address
    pub entry: u64,
    /// Program header offset
    #[allow(dead_code)]
    pub phoff: u64,
    /// Program header entry size
    #[allow(dead_code)]
    pub phentsize: u16,
    /// Number of program headers
    #[allow(dead_code)]
    pub phnum: u16,
}

impl ElfHeader {
    /// Parse ELF header from a file
    pub fn from_file(file: &mut File) -> Result<Self> {
        file.seek(SeekFrom::Start(0))?;

        let mut ident = [0u8; 16];
        file.read_exact(&mut ident)?;

        // Check magic
        if &ident[0..4] != ELF_MAGIC {
            bail!("Not an ELF file");
        }

        // Check class
        let class = match ident[4] {
            1 => ElfClass::Elf32,
            2 => ElfClass::Elf64,
            _ => bail!("Invalid ELF class"),
        };

        // Only support 64-bit for now
        if class != ElfClass::Elf64 {
            bail!("Only 64-bit ELF files are supported");
        }

        // Read rest of header (64-bit)
        let mut buf = vec![0u8; 48]; // Rest of 64-bit ELF header
        file.read_exact(&mut buf)?;

        let entry = u64::from_le_bytes(buf[8..16].try_into()?);
        let phoff = u64::from_le_bytes(buf[16..24].try_into()?);
        let phentsize = u16::from_le_bytes(buf[38..40].try_into()?);
        let phnum = u16::from_le_bytes(buf[40..42].try_into()?);

        Ok(Self {
            class,
            entry,
            phoff,
            phentsize,
            phnum,
        })
    }

    /// Update entry point in the file
    #[allow(dead_code)]
    pub fn update_entry_point(file: &mut File, new_entry: u64) -> Result<()> {
        // Entry point is at offset 24 in 64-bit ELF header
        file.seek(SeekFrom::Start(24))?;
        file.write_all(&new_entry.to_le_bytes())?;
        Ok(())
    }
}

/// Program header type
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum ProgType {
    Null = 0,
    Load = 1,
    Dynamic = 2,
    Interp = 3,
    Note = 4,
    Phdr = 6,
}

/// Program header
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

impl ProgramHeader {
    /// Size of a 64-bit program header
    pub const SIZE: usize = 56;

    /// Parse a program header from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < Self::SIZE {
            bail!("Program header too short");
        }

        Ok(Self {
            p_type: u32::from_le_bytes(bytes[0..4].try_into()?),
            p_flags: u32::from_le_bytes(bytes[4..8].try_into()?),
            p_offset: u64::from_le_bytes(bytes[8..16].try_into()?),
            p_vaddr: u64::from_le_bytes(bytes[16..24].try_into()?),
            p_paddr: u64::from_le_bytes(bytes[24..32].try_into()?),
            p_filesz: u64::from_le_bytes(bytes[32..40].try_into()?),
            p_memsz: u64::from_le_bytes(bytes[40..48].try_into()?),
            p_align: u64::from_le_bytes(bytes[48..56].try_into()?),
        })
    }

    /// Convert to bytes
    #[allow(dead_code)]
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut bytes = [0u8; Self::SIZE];
        bytes[0..4].copy_from_slice(&self.p_type.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.p_flags.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.p_offset.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.p_vaddr.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.p_paddr.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.p_filesz.to_le_bytes());
        bytes[40..48].copy_from_slice(&self.p_memsz.to_le_bytes());
        bytes[48..56].copy_from_slice(&self.p_align.to_le_bytes());
        bytes
    }
}

/// Read all program headers from an ELF file
#[allow(dead_code)]
pub fn read_program_headers(file: &mut File, header: &ElfHeader) -> Result<Vec<ProgramHeader>> {
    let mut headers = Vec::new();

    file.seek(SeekFrom::Start(header.phoff))?;

    for _ in 0..header.phnum {
        let mut buf = vec![0u8; ProgramHeader::SIZE];
        file.read_exact(&mut buf)?;
        headers.push(ProgramHeader::from_bytes(&buf)?);
    }

    Ok(headers)
}

/// Check if a file is an ELF executable
#[allow(dead_code)]
pub fn is_elf_file(path: &Path) -> Result<bool> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 4];

    match file.read_exact(&mut magic) {
        Ok(()) => Ok(&magic == ELF_MAGIC),
        Err(_) => Ok(false),
    }
}

/// Validate that a file is a supported ELF executable
pub fn validate_elf(path: &Path) -> Result<()> {
    let mut file = File::open(path)?;
    let header = ElfHeader::from_file(&mut file).context("Invalid ELF file")?;

    if header.class != ElfClass::Elf64 {
        bail!("Only 64-bit ELF files are supported");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elf_magic() {
        assert_eq!(ELF_MAGIC, b"\x7fELF");
    }

    #[test]
    fn test_program_header_serialization() {
        let ph = ProgramHeader {
            p_type: 1,
            p_flags: 5,
            p_offset: 0x1000,
            p_vaddr: 0x400000,
            p_paddr: 0x400000,
            p_filesz: 0x2000,
            p_memsz: 0x2000,
            p_align: 0x1000,
        };

        let bytes = ph.to_bytes();
        let parsed = ProgramHeader::from_bytes(&bytes).unwrap();

        assert_eq!(parsed.p_type, ph.p_type);
        assert_eq!(parsed.p_flags, ph.p_flags);
        assert_eq!(parsed.p_offset, ph.p_offset);
        assert_eq!(parsed.p_vaddr, ph.p_vaddr);
    }
}
