//! Example demonstrating how to use packelf programmatically

use std::process::Command;

fn main() {
    println!("PackELF Example");
    println!("===============\n");

    // This example shows how to use the packelf CLI tool
    // In a real application, you would use the packelf library functions directly

    let packelf = env!("CARGO_BIN_EXE_packelf");

    println!("1. Creating a test binary...");
    let test_src = r#"
        fn main() {
            println!("Hello from packed executable!");
            for (i, arg) in std::env::args().enumerate() {
                println!("  arg[{}] = {}", i, arg);
            }
        }
    "#;

    std::fs::write("/tmp/test_program.rs", test_src).expect("Failed to write test source");

    let status = Command::new("rustc")
        .args(["/tmp/test_program.rs", "-o", "/tmp/test_program"])
        .status()
        .expect("Failed to compile test program");

    if !status.success() {
        eprintln!("Failed to compile test program");
        return;
    }

    println!("   Created /tmp/test_program\n");

    // Get original size
    let original_size = std::fs::metadata("/tmp/test_program")
        .expect("Failed to get metadata")
        .len();
    println!("   Original size: {} bytes\n", original_size);

    println!("2. Packing the binary...");
    let output = Command::new(packelf)
        .args(["pack", "/tmp/test_program", "-f"])
        .output()
        .expect("Failed to pack");

    println!("{}", String::from_utf8_lossy(&output.stdout));

    if !output.status.success() {
        eprintln!("Packing failed: {}", String::from_utf8_lossy(&output.stderr));
        return;
    }

    let packed_size = std::fs::metadata("/tmp/test_program.bin.packed")
        .expect("Failed to get packed metadata")
        .len();

    println!("\n3. Showing info about packed file...");
    let output = Command::new(packelf)
        .args(["info", "/tmp/test_program.bin.packed"])
        .output()
        .expect("Failed to show info");

    println!("{}", String::from_utf8_lossy(&output.stdout));

    println!("\n4. Testing packed file detection...");
    let output = Command::new(packelf)
        .args(["test", "/tmp/test_program.bin.packed"])
        .output()
        .expect("Failed to test");

    println!("{}", String::from_utf8_lossy(&output.stdout));

    println!("\n5. Unpacking the binary...");
    let output = Command::new(packelf)
        .args(["unpack", "/tmp/test_program.bin.packed", "-f"])
        .output()
        .expect("Failed to unpack");

    println!("{}", String::from_utf8_lossy(&output.stdout));

    if !output.status.success() {
        eprintln!("Unpacking failed: {}", String::from_utf8_lossy(&output.stderr));
        return;
    }

    println!("\n6. Verifying unpacked binary...");
    let output = Command::new("/tmp/test_program.bin")
        .args(["arg1", "arg2", "arg3"])
        .output()
        .expect("Failed to run unpacked binary");

    println!("{}", String::from_utf8_lossy(&output.stdout));

    // Verify checksums match
    let original_hash = sha256_file("/tmp/test_program");
    let unpacked_hash = sha256_file("/tmp/test_program.bin");

    println!("\n7. Verification:");
    println!("   Original checksum:  {}", original_hash);
    println!("   Unpacked checksum:  {}", unpacked_hash);
    println!("   Match: {}", original_hash == unpacked_hash);

    println!(
        "\n8. Compression summary:");
    println!("   Original:  {} bytes", original_size);
    println!("   Packed:    {} bytes", packed_size);
    println!(
        "   Savings:   {} bytes ({:.1}%)",
        original_size - packed_size,
        ((original_size - packed_size) as f64 / original_size as f64) * 100.0
    );

    // Cleanup
    let _ = std::fs::remove_file("/tmp/test_program.rs");
    let _ = std::fs::remove_file("/tmp/test_program");
    let _ = std::fs::remove_file("/tmp/test_program.bin");
    let _ = std::fs::remove_file("/tmp/test_program.bin.packed");
}

fn sha256_file(path: &str) -> String {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("Failed to compute checksum");

    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}
