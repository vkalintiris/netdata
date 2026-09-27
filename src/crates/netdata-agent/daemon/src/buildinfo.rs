//! `BUILD_INFO` (`src/daemon/buildinfo.c`): the 119 slots of `-W buildinfo`, `-W buildinfojson` and (later)
//! `/api/v2/info`'s `application`, each saying what the Rust agent provides in C's vocabulary (D87.1: YES only for what
//! it provides; check `cli.buildinfo` lists where that still differs from C's production build), with C's text, JSON
//! and analytics renderings, and C's cmake cache for `-W cmakecache` (D87.2).

use std::io::Read;

use netdata_agent_rrd::system_info::SystemInfo;
use netdata_agent_text::json::{JsonOptions, JsonWriter};

use crate::build;
use crate::conf::Dirs;
use crate::system::SystemMemory;

/// `BUILD_INFO_CATEGORY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Packaging,
    Directories,
    OperatingSystem,
    Hardware,
    Container,
    Feature,
    Database,
    Connectivity,
    Libs,
    Plugins,
    Exporters,
    DebugDevel,
    Runtime,
}

/// The categories in C's output order: the text title and the JSON key of each.
const SECTIONS: [(Category, &str, &str); 13] = [
    (Category::Packaging, "Packaging", "package"),
    (Category::Directories, "Default Directories", "directories"),
    (Category::OperatingSystem, "Operating System", "os"),
    (Category::Hardware, "Hardware", "hw"),
    (Category::Container, "Container", "container"),
    (Category::Feature, "Features", "features"),
    (Category::Database, "Database Engines", "databases"),
    (Category::Connectivity, "Connectivity Capabilities", "connectivity"),
    (Category::Libs, "Libraries", "libs"),
    (Category::Plugins, "Plugins", "plugins"),
    (Category::Exporters, "Exporters", "exporters"),
    (Category::DebugDevel, "Debug/Developer Features", "debug-n-devel"),
    (Category::Runtime, "Runtime Information", "runtime"),
];

/// `BUILD_INFO_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Boolean,
    String,
}

/// A slot's constant part.
struct Def {
    category: Category,
    kind: Kind,
    #[cfg_attr(not(test), expect(dead_code, reason = "read by analytics(), whose consumer is /api/v1/info's tail"))]
    analytics: Option<&'static str>,
    print: &'static str,
    json: &'static str,
}

const fn def(
    category: Category,
    kind: Kind,
    analytics: Option<&'static str>,
    print: &'static str,
    json: &'static str,
) -> Def {
    Def {
        category,
        kind,
        analytics,
        print,
        json,
    }
}

/// `BUILD_INFO[]`, in C's slot order (which is the output order).
#[rustfmt::skip]
const DEFS: [Def; 119] = [
    def(Category::Packaging, Kind::String, None, "Netdata Version", "version"),
    def(Category::Packaging, Kind::String, None, "Installation Type", "type"),
    def(Category::Packaging, Kind::String, None, "Package Architecture", "arch"),
    def(Category::Packaging, Kind::String, None, "Package Distro", "distro"),
    def(Category::Packaging, Kind::String, None, "Configure Options", "configure"),
    def(Category::Directories, Kind::String, None, "User Configurations", "user_config"),
    def(Category::Directories, Kind::String, None, "Stock Configurations", "stock_config"),
    def(Category::Directories, Kind::String, None, "Stock Data Files", "stock_data"),
    def(Category::Directories, Kind::String, None, "Ephemeral Databases (metrics data, metadata)", "ephemeral_db"),
    def(Category::Directories, Kind::String, None, "Permanent Databases", "permanent_db"),
    def(Category::Directories, Kind::String, None, "Plugins", "plugins"),
    def(Category::Directories, Kind::String, None, "Static Web Files", "web"),
    def(Category::Directories, Kind::String, None, "Log Files", "logs"),
    def(Category::Directories, Kind::String, None, "Lock Files", "locks"),
    def(Category::Directories, Kind::String, None, "Home", "home"),
    def(Category::OperatingSystem, Kind::String, None, "Kernel", "kernel"),
    def(Category::OperatingSystem, Kind::String, None, "Kernel Version", "kernel_version"),
    def(Category::OperatingSystem, Kind::String, None, "Operating System", "os"),
    def(Category::OperatingSystem, Kind::String, None, "Operating System ID", "id"),
    def(Category::OperatingSystem, Kind::String, None, "Operating System ID Like", "id_like"),
    def(Category::OperatingSystem, Kind::String, None, "Operating System Version", "version"),
    def(Category::OperatingSystem, Kind::String, None, "Operating System Version ID", "version_id"),
    def(Category::OperatingSystem, Kind::String, None, "Detection", "detection"),
    def(Category::Hardware, Kind::String, None, "CPU Cores", "cpu_cores"),
    def(Category::Hardware, Kind::String, None, "CPU Frequency", "cpu_frequency"),
    def(Category::Hardware, Kind::String, None, "RAM Bytes", "ram"),
    def(Category::Hardware, Kind::String, None, "Disk Capacity", "disk"),
    def(Category::Hardware, Kind::String, None, "CPU Architecture", "cpu_architecture"),
    def(Category::Hardware, Kind::String, None, "Virtualization Technology", "virtualization"),
    def(Category::Hardware, Kind::String, None, "Virtualization Detection", "virtualization_detection"),
    def(Category::Container, Kind::String, None, "Container", "container"),
    def(Category::Container, Kind::String, None, "Container Detection", "container_detection"),
    def(Category::Container, Kind::String, None, "Container Orchestrator", "orchestrator"),
    def(Category::Container, Kind::String, None, "Container Operating System", "os"),
    def(Category::Container, Kind::String, None, "Container Operating System ID", "os_id"),
    def(Category::Container, Kind::String, None, "Container Operating System ID Like", "os_id_like"),
    def(Category::Container, Kind::String, None, "Container Operating System Version", "version"),
    def(Category::Container, Kind::String, None, "Container Operating System Version ID", "version_id"),
    def(Category::Container, Kind::String, None, "Container Operating System Detection", "detection"),
    def(Category::Feature, Kind::String, None, "Built For", "built-for"),
    def(Category::Feature, Kind::Boolean, Some("Netdata Cloud"), "Netdata Cloud", "cloud"),
    def(Category::Feature, Kind::Boolean, None, "Health (trigger alerts and send notifications)", "health"),
    def(Category::Feature, Kind::Boolean, None, "Streaming (stream metrics to parent Netdata servers)", "streaming"),
    def(Category::Feature, Kind::Boolean, None, "Back-filling (of higher database tiers)", "back-filling"),
    def(Category::Feature, Kind::Boolean, None, "Replication (fill the gaps of parent Netdata servers)", "replication"),
    def(Category::Feature, Kind::Boolean, Some("Stream Compression"), "Streaming and Replication Compression", "stream-compression"),
    def(Category::Feature, Kind::Boolean, None, "Contexts (index all active and archived metrics)", "contexts"),
    def(Category::Feature, Kind::Boolean, None, "Tiering (multiple dbs with different metrics resolution)", "tiering"),
    def(Category::Feature, Kind::Boolean, Some("Machine Learning"), "Machine Learning", "ml"),
    def(Category::Feature, Kind::String, Some("allocator"), "Memory Allocator", "allocator"),
    def(Category::Database, Kind::String, None, "sqlite", "sqlite"),
    def(Category::Database, Kind::Boolean, Some("dbengine"), "dbengine (compression)", "dbengine"),
    def(Category::Database, Kind::Boolean, None, "alloc", "alloc"),
    def(Category::Database, Kind::Boolean, None, "ram", "ram"),
    def(Category::Database, Kind::Boolean, None, "none", "none"),
    def(Category::Connectivity, Kind::Boolean, None, "ACLK (Agent-Cloud Link: MQTT over WebSockets over TLS)", "aclk"),
    def(Category::Connectivity, Kind::Boolean, None, "static (Netdata internal web server)", "static"),
    def(Category::Connectivity, Kind::Boolean, None, "WebRTC (experimental)", "webrtc"),
    def(Category::Connectivity, Kind::Boolean, Some("Native HTTPS"), "Native HTTPS (TLS Support)", "native-https"),
    def(Category::Connectivity, Kind::Boolean, Some("TLS Host Verification"), "TLS Host Verification", "tls-host-verify"),
    def(Category::Libs, Kind::Boolean, None, "LZ4 (extremely fast lossless compression algorithm)", "lz4"),
    def(Category::Libs, Kind::Boolean, None, "ZSTD (fast, lossless compression algorithm)", "zstd"),
    def(Category::Libs, Kind::Boolean, Some("zlib"), "zlib (lossless data-compression library)", "zlib"),
    def(Category::Libs, Kind::Boolean, None, "Brotli (generic-purpose lossless compression algorithm)", "brotli"),
    def(Category::Libs, Kind::Boolean, Some("protobuf"), "protobuf (platform-neutral data serialization protocol)", "protobuf"),
    def(Category::Libs, Kind::Boolean, None, "OpenSSL (cryptography)", "openssl"),
    def(Category::Libs, Kind::Boolean, None, "libdatachannel (stand-alone WebRTC data channels)", "libdatachannel"),
    def(Category::Libs, Kind::Boolean, Some("JSON-C"), "JSON-C (lightweight JSON manipulation)", "jsonc"),
    def(Category::Libs, Kind::Boolean, Some("libcap"), "libcap (Linux capabilities system operations)", "libcap"),
    def(Category::Libs, Kind::Boolean, Some("libcrypto"), "libcrypto (cryptographic functions)", "libcrypto"),
    def(Category::Libs, Kind::Boolean, Some("libyaml"), "libyaml (library for parsing and emitting YAML)", "libyaml"),
    def(Category::Libs, Kind::Boolean, Some("libmnl"), "libmnl (library for working with netfilter)", "libmnl"),
    def(Category::Libs, Kind::String, Some("stacktraces"), "stacktraces (library for getting stack traces)", "stacktraces"),
    def(Category::Plugins, Kind::Boolean, Some("apps"), "apps (monitor processes)", "apps"),
    def(Category::Plugins, Kind::Boolean, None, "cgroups (monitor containers and VMs)", "cgroups"),
    def(Category::Plugins, Kind::Boolean, Some("cgroup Network Tracking"), "cgroup-network (associate interfaces to CGROUPS)", "cgroup-network"),
    def(Category::Plugins, Kind::Boolean, None, "proc (monitor Linux systems)", "proc"),
    def(Category::Plugins, Kind::Boolean, None, "tc (monitor Linux network QoS)", "tc"),
    def(Category::Plugins, Kind::Boolean, None, "diskspace (monitor Linux mount points)", "diskspace"),
    def(Category::Plugins, Kind::Boolean, None, "freebsd (monitor FreeBSD systems)", "freebsd"),
    def(Category::Plugins, Kind::Boolean, None, "macos (monitor MacOS systems)", "macos"),
    def(Category::Plugins, Kind::Boolean, Some("MACOS-LOGS"), "macos-logs (monitor macOS unified logs)", "macos-logs"),
    def(Category::Plugins, Kind::Boolean, None, "windows (monitor Windows systems)", "windows"),
    def(Category::Plugins, Kind::Boolean, None, "statsd (collect custom application metrics)", "statsd"),
    def(Category::Plugins, Kind::Boolean, None, "timex (check system clock synchronization)", "timex"),
    def(Category::Plugins, Kind::Boolean, None, "idlejitter (check system latency and jitter)", "idlejitter"),
    def(Category::Plugins, Kind::Boolean, None, "bash (support shell data collection jobs - charts.d)", "charts.d"),
    def(Category::Plugins, Kind::Boolean, Some("debugfs"), "debugfs (kernel debugging metrics)", "debugfs"),
    def(Category::Plugins, Kind::Boolean, Some("CUPS"), "cups (monitor printers and print jobs)", "cups"),
    def(Category::Plugins, Kind::Boolean, Some("EBPF"), "ebpf (monitor system calls)", "ebpf"),
    def(Category::Plugins, Kind::Boolean, Some("IPMI"), "freeipmi (monitor enterprise server H/W)", "freeipmi"),
    def(Category::Plugins, Kind::Boolean, Some("NETWORK-VIEWER"), "network-viewer (monitor TCP/UDP IPv4/6 sockets)", "network-viewer"),
    def(Category::Plugins, Kind::Boolean, Some("SYSTEMD-JOURNAL"), "systemd-journal (monitor journal logs)", "systemd-journal"),
    def(Category::Plugins, Kind::Boolean, Some("WINDOWS-EVENTS"), "windows-events (monitor Windows events)", "windows-events"),
    def(Category::Plugins, Kind::Boolean, Some("NFACCT"), "nfacct (gather netfilter accounting)", "nfacct"),
    def(Category::Plugins, Kind::Boolean, Some("perf"), "perf (collect kernel performance events)", "perf"),
    def(Category::Plugins, Kind::Boolean, Some("slabinfo"), "slabinfo (monitor kernel object caching)", "slabinfo"),
    def(Category::Plugins, Kind::Boolean, Some("Xen"), "Xen", "xen"),
    def(Category::Plugins, Kind::Boolean, Some("Xen VBD Error Tracking"), "Xen VBD Error Tracking", "xen-vbd-error"),
    def(Category::Exporters, Kind::Boolean, Some("AWS Kinesis"), "AWS Kinesis", "kinesis"),
    def(Category::Exporters, Kind::Boolean, Some("GCP PubSub"), "GCP PubSub", "pubsub"),
    def(Category::Exporters, Kind::Boolean, Some("MongoDB"), "MongoDB", "mongodb"),
    def(Category::Exporters, Kind::Boolean, None, "Prometheus (OpenMetrics) Exporter", "openmetrics"),
    def(Category::Exporters, Kind::Boolean, Some("Prometheus Remote Write"), "Prometheus Remote Write", "prom-remote-write"),
    def(Category::Exporters, Kind::Boolean, None, "Graphite", "graphite"),
    def(Category::Exporters, Kind::Boolean, None, "Graphite HTTP / HTTPS", "graphite:http"),
    def(Category::Exporters, Kind::Boolean, None, "JSON", "json"),
    def(Category::Exporters, Kind::Boolean, None, "JSON HTTP / HTTPS", "json:http"),
    def(Category::Exporters, Kind::Boolean, None, "OpenTSDB", "opentsdb"),
    def(Category::Exporters, Kind::Boolean, None, "OpenTSDB HTTP / HTTPS", "opentsdb:http"),
    def(Category::Exporters, Kind::Boolean, None, "All Metrics API", "allmetrics"),
    def(Category::Exporters, Kind::Boolean, None, "Shell (use metrics in shell scripts)", "shell"),
    def(Category::DebugDevel, Kind::Boolean, Some("DebugTraceAlloc"), "Trace All Netdata Allocations (with charts)", "trace-allocations"),
    def(Category::DebugDevel, Kind::Boolean, None, "Developer Mode (more runtime checks, slower)", "dev-mode"),
    def(Category::Runtime, Kind::String, Some("ConfigProfile"), "Profile", "profile"),
    def(Category::Runtime, Kind::Boolean, Some("StreamParent"), "Stream Parent (accept data from Children)", "parent"),
    def(Category::Runtime, Kind::Boolean, Some("StreamChild"), "Stream Child (send data to a Parent)", "child"),
    def(Category::Runtime, Kind::String, Some("TotalMemory"), "Total System Memory", "mem-total"),
    def(Category::Runtime, Kind::String, Some("AvailableMemory"), "Available System Memory", "mem-available"),
];

/// A slot's state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Slot {
    status: bool,
    value: Option<String>,
}

/// What the slots are filled from besides the build: C's `populate_packaging_info()`, `populate_system_info()` and
/// `populate_directories()`.
#[derive(Debug, Clone)]
pub struct Inputs<'a> {
    pub dirs: &'a Dirs,
    /// `netdata_configured_home_dir` (the compile-time one until the startup's `home` step).
    pub home: &'a str,
    /// A `system-info.sh` detection with the install type.
    pub system: &'a SystemInfo,
    /// The profile's text (`ND_PROFILE_2buffer()`).
    pub profile: &'a str,
    pub parent: bool,
    pub child: bool,
    /// `os_system_memory(true)`.
    pub memory: SystemMemory,
}

/// The slots, filled.
#[derive(Debug, Clone)]
pub struct BuildInfo {
    slots: Vec<Slot>,
}

/// `CONFIGURE_COMMAND`: the Rust build's command, from the build (D87.2), else C's initializer.
const CONFIGURE_COMMAND: &str = match option_env!("NETDATA_CONFIGURE_COMMAND") {
    Some(command) => command,
    None => "unknown",
};

impl BuildInfo {
    fn slot(&mut self, category: Category, json: &str) -> &mut Slot {
        let i = DEFS
            .iter()
            .position(|d| d.category == category && d.json == json)
            .unwrap_or_else(|| panic!("no build info slot {json}"));
        &mut self.slots[i]
    }

    fn yes(&mut self, category: Category, keys: &[&str]) {
        for key in keys {
            self.slot(category, key).status = true;
        }
    }

    fn set(&mut self, category: Category, key: &str, value: &str) {
        self.slot(category, key).value = Some(value.to_string());
    }

    /// `build_info_set_value_strdupz()`: a missing value is the empty string.
    fn set_opt(&mut self, category: Category, key: &str, value: Option<&str>) {
        self.set(category, key, value.unwrap_or(""));
    }

    /// `build_info_append_value()`: the words joined by spaces.
    fn append(&mut self, category: Category, key: &str, value: &str) {
        let slot = self.slot(category, key);
        slot.value = Some(match slot.value.take() {
            Some(old) => format!("{old} {value}"),
            None => value.to_string(),
        });
    }

    /// `initialize_build_info()` for what the Rust agent provides (D87.1), then the populate steps.
    pub fn new(inputs: &Inputs<'_>) -> BuildInfo {
        use Category::*;
        let mut b = BuildInfo {
            slots: vec![Slot::default(); DEFS.len()],
        };
        b.set(Packaging, "version", build::NETDATA_VERSION);
        b.set(Packaging, "configure", CONFIGURE_COMMAND);
        b.yes(Feature, &["built-for"]);
        b.set(Feature, "built-for", "Linux");
        // the receivers decompress every algorithm C's build has; sending waits for the stream sender (M7)
        b.yes(Feature, &["back-filling", "stream-compression", "contexts", "tiering", "allocator"]);
        for algorithm in ["zstd", "lz4", "gzip", "brotli"] {
            b.append(Feature, "stream-compression", algorithm);
        }
        b.set(Feature, "tiering", "5");
        b.set(Feature, "allocator", "system");
        b.set(Database, "sqlite", netdata_agent_metadata::library::version());
        b.yes(Database, &["dbengine", "alloc", "ram", "none"]);
        b.append(Database, "dbengine", "zstd");
        b.append(Database, "dbengine", "lz4");
        b.yes(Connectivity, &["static"]);
        // brotli decodes only: every role the agent plays today (D87.8, Q2)
        b.yes(Libs, &["lz4", "zstd", "brotli", "zlib", "jsonc"]);

        // populate_packaging_info()
        let unknown = |v: &Option<String>| v.clone().unwrap_or_else(|| "unknown".to_string());
        b.set(Packaging, "type", &unknown(&inputs.system.install_type));
        b.set(Packaging, "arch", &unknown(&inputs.system.prebuilt_arch));
        b.set(Packaging, "distro", &unknown(&inputs.system.prebuilt_dist));
        b.set(Runtime, "profile", inputs.profile);
        b.slot(Runtime, "parent").status = inputs.parent;
        b.slot(Runtime, "child").status = inputs.child;
        if inputs.memory.total > 0 {
            b.set(Runtime, "mem-total", &inputs.memory.total.to_string());
            b.set(Runtime, "mem-available", &inputs.memory.available.to_string());
        }

        // populate_system_info()
        let si = inputs.system;
        for (category, key, value) in [
            (OperatingSystem, "kernel", &si.kernel_name),
            (OperatingSystem, "kernel_version", &si.kernel_version),
            (OperatingSystem, "os", &si.host_os_name),
            (OperatingSystem, "id", &si.host_os_id),
            (OperatingSystem, "id_like", &si.host_os_id_like),
            (OperatingSystem, "version", &si.host_os_version),
            (OperatingSystem, "version_id", &si.host_os_version_id),
            (OperatingSystem, "detection", &si.host_os_detection),
            (Hardware, "cpu_cores", &si.host_cores),
            (Hardware, "cpu_frequency", &si.host_cpu_freq),
            (Hardware, "ram", &si.host_ram_total),
            (Hardware, "disk", &si.host_disk_space),
            (Hardware, "cpu_architecture", &si.architecture),
            (Hardware, "virtualization", &si.virtualization),
            (Hardware, "virtualization_detection", &si.virt_detection),
            (Container, "container", &si.container),
            (Container, "container_detection", &si.container_detection),
            (Container, "os", &si.container_os_name),
            (Container, "os_id", &si.container_os_id),
            (Container, "os_id_like", &si.container_os_id_like),
            (Container, "version", &si.container_os_version),
            (Container, "version_id", &si.container_os_version_id),
            (Container, "detection", &si.container_os_detection),
        ] {
            b.set_opt(category, key, value.as_deref());
        }
        let orchestrator = if si.is_k8s_node.as_deref() == Some("true") {
            "kubernetes"
        } else {
            "none"
        };
        b.set(Container, "orchestrator", orchestrator);

        // populate_directories(); the lock directory stays the compile-time one
        let dirs = inputs.dirs;
        for (key, value) in [
            ("user_config", dirs.user_config.as_str()),
            ("stock_config", &dirs.stock_config),
            ("stock_data", &dirs.stock_data),
            ("ephemeral_db", &dirs.cache),
            ("permanent_db", &dirs.varlib),
            ("plugins", dirs.plugins.first().map_or("(null)", String::as_str)),
            ("web", &dirs.web),
            ("logs", &dirs.log),
            ("locks", &format!("{}/lock", build::VARLIB_DIR)),
            ("home", inputs.home),
        ] {
            b.set(Directories, key, value);
        }
        b
    }

    fn category(&self, category: Category) -> impl Iterator<Item = (&Def, &Slot)> {
        DEFS.iter()
            .zip(&self.slots)
            .filter(move |(d, _)| d.category == category)
    }

    /// `print_build_info()`: each category's title, then its slots, their labels padded to 60 columns.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (category, title, _) in SECTIONS {
            out.push_str(title);
            out.push_str(":\n");
            for (d, s) in self.category(category) {
                let padding = "_".repeat(60usize.saturating_sub(d.print.len() + 1));
                let shown = match (d.kind, &s.value) {
                    (Kind::String, value) => value.as_deref().unwrap_or("unknown").to_string(),
                    (Kind::Boolean, value) => {
                        let yes = if s.status { "YES" } else { "NO" };
                        match value {
                            Some(v) => format!("{yes} ({v})"),
                            None => yes.to_string(),
                        }
                    }
                };
                out.push_str(&format!("    {} {padding} : {shown}\n", d.print));
            }
        }
        out
    }

    /// `build_info_to_json_object()`: each category as an object of its slots, a value as a string, else the status.
    pub fn to_json_object(&self, w: &mut JsonWriter) {
        for (category, _, key) in SECTIONS {
            w.member_add_object(key);
            for (d, s) in self.category(category) {
                match &s.value {
                    Some(v) => w.member_add_string(d.json, v),
                    None => w.member_add_boolean(d.json, s.status),
                }
            }
            w.object_close();
        }
    }

    /// `print_build_info_json()`: the object, then C's `printf("%s\n")`.
    pub fn json(&self) -> Vec<u8> {
        let mut w = JsonWriter::new(JsonOptions::DEFAULT);
        self.to_json_object(&mut w);
        w.finalize();
        let mut out = w.into_bytes();
        out.push(b'\n');
        out
    }

    /// `analytics_build_info()`: the analytics names of the slots that hold, joined by `|`.
    #[cfg_attr(not(test), expect(dead_code, reason = "for /api/v1/info's buildinfo, with the info tail (D84.2)"))]
    pub fn analytics(&self) -> String {
        DEFS.iter()
            .zip(&self.slots)
            .filter(|(d, s)| d.analytics.is_some() && s.status)
            .filter_map(|(d, _)| d.analytics)
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// `print_build_info_cmake_cache()`: C's cmake cache, unpacked, or C's message when it cannot be opened. A cache that
/// stops decoding prints what came before (D87.8, Q5: plain files are not read, as zlib's `gzopen` would).
pub fn cmake_cache() -> Vec<u8> {
    let path = format!("{}/build-info-cmake-cache.gz", build::STOCK_DATA_DIR);
    let Ok(file) = std::fs::File::open(&path) else {
        return format!("Could not open build info cmake cache archive located at {path}\n").into_bytes();
    };
    let mut out = Vec::new();
    let _ = flate2::read::MultiGzDecoder::new(file).read_to_end(&mut out);
    out
}

#[cfg(test)]
mod tests;
