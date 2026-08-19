//! Bakes in the same install-time paths the shell script got from CMake
//! (`@configdir_POST@`, `@libconfigdir_POST@`, `@cachedir_POST@`,
//! `@registrydir_POST@`). At runtime the daemon's environment variables win; these
//! are only the fallbacks for running the binary by hand.

use std::env;

fn emit(name: &str, fallback: &str) {
    println!("cargo:rerun-if-env-changed={name}");
    let value = env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string());
    println!("cargo:rustc-env={name}={value}");
}

fn main() {
    emit("NETDATA_BUILD_CONFIG_DIR", "/etc/netdata");
    emit("NETDATA_BUILD_STOCK_CONFIG_DIR", "/usr/lib/netdata/conf.d");
    emit("NETDATA_BUILD_CACHE_DIR", "/var/cache/netdata");
    emit("NETDATA_BUILD_REGISTRY_DIR", "/var/lib/netdata/registry");
}
