use super::*;

fn inputs<'a>(dirs: &'a Dirs, si: &'a SystemInfo, memory: SystemMemory) -> Inputs<'a> {
    Inputs {
        dirs,
        home: "/var/lib/netdata",
        system: si,
        profile: "standalone",
        parent: false,
        child: false,
        memory,
    }
}

fn system_info() -> SystemInfo {
    SystemInfo {
        kernel_name: Some("Linux".into()),
        host_cores: Some("16".into()),
        is_k8s_node: Some("false".into()),
        install_type: Some("custom".into()),
        ..SystemInfo::default()
    }
}

const MEMORY: SystemMemory = SystemMemory {
    total: 33659879424,
    available: 19488387072,
};

/// C's layout: 13 titles and 119 slots, labels padded with underscores to 60 columns, strings shown or "unknown",
/// booleans YES or NO with their value in parentheses; a missing detection field shows empty.
#[test]
fn the_text_as_c() {
    let (dirs, si) = (Dirs::default(), system_info());
    let text = BuildInfo::new(&inputs(&dirs, &si, MEMORY)).text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 132);
    assert!(text.ends_with('\n'));
    assert_eq!(lines[0], "Packaging:");
    let find = |label: &str| {
        *lines
            .iter()
            .find(|l| l.starts_with(&format!("    {label} ")))
            .unwrap()
    };
    assert_eq!(
        find("Installation Type"),
        format!("    Installation Type {} : custom", "_".repeat(60 - 17 - 1))
    );
    assert!(find("Package Architecture").ends_with(" : unknown"));
    assert!(find("Kernel").ends_with("_ : Linux"));
    assert!(find("Kernel Version").ends_with("_ : "));
    assert!(find("Container Orchestrator").ends_with(" : none"));
    assert!(find("Streaming and Replication Compression").ends_with(" : YES (zstd lz4 gzip brotli)"));
    for label in [
        "Streaming (stream metrics to parent Netdata servers)",
        "Replication (fill the gaps of parent Netdata servers)",
        "Native HTTPS (TLS Support)",
        "TLS Host Verification",
        "OpenSSL (cryptography)",
        "libcrypto (cryptographic functions)",
    ] {
        assert!(find(label).ends_with(" : YES"), "{label}");
    }
    assert!(find("Tiering (multiple dbs with different metrics resolution)").ends_with(" : YES (5)"));
    assert!(find("Netdata Cloud").ends_with(" : NO"));
    assert!(find("stacktraces (library for getting stack traces)").ends_with(" : unknown"));
    assert!(find("Lock Files").ends_with(" : /var/lib/netdata/lock"));
    assert_eq!(*lines.last().unwrap(), format!("    Available System Memory {} : 19488387072", "_".repeat(36)));
}

/// C's JSON: a value prints as a string, whatever the slot's type; a slot without one prints its status; the
/// sections in C's order; C's ending, `printf("%s\n")` after the finalized buffer.
#[test]
fn the_json_as_c() {
    let (dirs, si) = (Dirs::default(), system_info());
    let json = String::from_utf8(BuildInfo::new(&inputs(&dirs, &si, MEMORY)).json()).unwrap();
    assert!(json.starts_with("{\n    \"package\":{\n        \"version\":\""));
    assert!(json.ends_with("    }\n}\n\n"));
    assert!(json.contains("\"tiering\":\"5\""));
    assert!(json.contains("\"stream-compression\":\"zstd lz4 gzip brotli\""));
    assert!(json.contains("\"cloud\":false"));
    for key in ["streaming", "replication", "native-https", "tls-host-verify", "openssl", "libcrypto"] {
        assert!(json.contains(&format!("\"{key}\":true")), "{key}");
    }
    assert!(json.contains("\"stacktraces\":false"));
    assert!(json.contains("\"kernel_version\":\"\""));
    let order: Vec<usize> = SECTIONS
        .iter()
        .map(|(_, _, key)| json.find(&format!("\"{key}\":{{")).unwrap())
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]));
}

/// Unknown memory: no value, so "unknown" in the text and `false` in JSON.
#[test]
fn unknown_memory_as_c() {
    let (dirs, si) = (Dirs::default(), system_info());
    let info = BuildInfo::new(&inputs(&dirs, &si, SystemMemory::default()));
    assert!(info.text().lines().last().unwrap().ends_with(" : unknown"));
    let json = String::from_utf8(info.json()).unwrap();
    assert!(json.contains("\"mem-total\":false,\n        \"mem-available\":false"));
}

/// The analytics names of the slots that hold, in slot order; the stream roles among them.
#[test]
fn the_analytics_join() {
    let (dirs, si) = (Dirs::default(), system_info());
    assert_eq!(
        BuildInfo::new(&inputs(&dirs, &si, MEMORY)).analytics(),
        "Stream Compression|allocator|dbengine|Native HTTPS|TLS Host Verification|zlib|JSON-C|libcrypto"
    );
    let parent = Inputs {
        parent: true,
        ..inputs(&dirs, &si, MEMORY)
    };
    assert!(BuildInfo::new(&parent).analytics().ends_with("|libcrypto|StreamParent"));
}

/// Every slot is named once per category (the setters find them by JSON key).
#[test]
fn slot_keys_are_unique() {
    for (i, a) in DEFS.iter().enumerate() {
        for b in &DEFS[i + 1..] {
            assert!(a.category != b.category || a.json != b.json, "{}", a.json);
        }
    }
}
