//! PackELF Native Stub - In-memory decompression and execution
//!
//! This stub is embedded in packed binaries and handles decompression
//! and execution without writing to disk using memfd_create + execveat.

use packelf_runtime::{PackElfHeader, decompress_and_verify};

fn main() {
    // Get our own executable path
    let self_path =
        std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("/proc/self/exe"));

    // Read our own binary
    let self_data = std::fs::read(&self_path)
        .unwrap_or_else(|e| fatal_error(&format!("Failed to read self: {}", e)));

    // Find and parse PackELF header at the end
    if self_data.len() < PackElfHeader::SIZE {
        fatal_error("File too small to contain PackELF header");
    }

    let header_offset = self_data.len() - PackElfHeader::SIZE;
    let header_bytes = &self_data[header_offset..];

    let header = PackElfHeader::from_bytes(header_bytes)
        .unwrap_or_else(|e| fatal_error(&format!("Invalid PackELF header: {}", e)));

    header
        .validate()
        .unwrap_or_else(|e| fatal_error(&format!("Header validation failed: {}", e)));

    // Extract compressed data
    let compressed_start = header.compressed_offset as usize;
    let compressed_end = compressed_start + header.compressed_size as usize;

    if compressed_end > header_offset {
        fatal_error("Invalid compressed data bounds");
    }

    let compressed_data = &self_data[compressed_start..compressed_end];

    // Decompress and verify
    let decompressed = decompress_and_verify(
        compressed_data,
        header.uncompressed_size as usize,
        header.checksum,
    )
    .unwrap_or_else(|e| fatal_error(&format!("Decompression failed: {}", e)));

    // Get original arguments
    let args: Vec<String> = std::env::args().collect();

    // Execute in-memory using memfd_create + execveat
    execute_memfd(&decompressed, &args)
        .unwrap_or_else(|e| fatal_error(&format!("Execution failed: {}", e)));

    // Should never reach here
    fatal_error("Unexpected: execution returned");
}

/// Execute using memfd_create + execveat (Linux 3.17+, truly in-memory)
fn execute_memfd(data: &[u8], args: &[String]) -> Result<(), String> {
    use nix::sys::memfd::{MFdFlags, memfd_create};
    use nix::unistd::write;
    use std::os::unix::io::AsRawFd;

    // Create anonymous memory file using nix
    let name = c"packelf";
    let fd = memfd_create(name, MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING)
        .map_err(|e| format!("memfd_create failed: {}", e))?;

    // Write decompressed data to memfd using nix
    let mut offset = 0;
    while offset < data.len() {
        match write(&fd, &data[offset..]) {
            Ok(n) => offset += n,
            Err(e) => {
                return Err(format!("Failed to write to memfd: {}", e));
            }
        }
    }

    let raw_fd = fd.as_raw_fd();

    // Execute using execveat
    execveat_fd(raw_fd, args)
}

/// Execute using execveat syscall via nix
fn execveat_fd(fd: std::os::unix::io::RawFd, args: &[String]) -> Result<(), String> {
    use nix::fcntl::AtFlags;
    use nix::unistd::execveat;
    use std::ffi::CString;
    use std::os::unix::io::BorrowedFd;

    // Convert args to C strings
    let c_args: Result<Vec<CString>, _> = args.iter().map(|s| CString::new(s.as_str())).collect();

    let c_args = c_args.map_err(|e| format!("Invalid argument: {}", e))?;

    // Get environment
    let env_vars: Result<Vec<CString>, _> = std::env::vars()
        .map(|(k, v)| CString::new(format!("{}={}", k, v)))
        .collect();

    let env_vars = env_vars.map_err(|e| format!("Invalid environment: {}", e))?;

    // Execute via nix's safe execveat wrapper
    let empty_path = CString::new("").unwrap();
    let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };

    execveat(
        borrowed_fd,
        &empty_path,
        &c_args,
        &env_vars,
        AtFlags::AT_EMPTY_PATH,
    )
    .map_err(|e| format!("execveat failed: {}", e))?;

    // Should never reach here
    Ok(())
}

fn fatal_error(msg: &str) -> ! {
    eprintln!("PackELF stub error: {}", msg);
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stub_compiles() {
        // Just verify the code compiles
        assert!(true);
    }
}
