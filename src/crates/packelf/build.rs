//! Build script to include the packelf-stub binary

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Tell cargo to rerun if the stub changes
    let stub_path = find_stub_binary();
    if let Some(path) = stub_path {
        println!("cargo:rerun-if-changed={}", path.display());

        // Copy stub to OUT_DIR so we can include it with a predictable path
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let dest = PathBuf::from(&out_dir).join("packelf-stub");

        if let Err(e) = std::fs::copy(&path, &dest) {
            println!("cargo:warning=Failed to copy stub: {}", e);
        } else {
            println!("cargo:warning=Using native packelf-stub from: {}", path.display());
            println!("cargo:rustc-cfg=feature=\"native_stub\"");
        }
    } else {
        println!("cargo:warning=packelf-stub not found, will use shell script fallback");
    }
}

fn find_stub_binary() -> Option<PathBuf> {
    // Try to find the packelf-stub binary in the target directory
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let workspace_dir = PathBuf::from(&manifest_dir).parent()?.to_path_buf();

    // Check release build first
    let release_stub = workspace_dir.join("target/release/packelf-stub");
    if release_stub.exists() {
        return Some(release_stub);
    }

    // Fall back to debug build
    let debug_stub = workspace_dir.join("target/debug/packelf-stub");
    if debug_stub.exists() {
        return Some(debug_stub);
    }

    None
}
