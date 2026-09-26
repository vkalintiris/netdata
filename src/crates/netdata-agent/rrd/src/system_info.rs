//! `struct rrdhost_system_info`, ported from `src/database/rrdhost-system-info.{h,c}`: what a host reports about its
//! operating system, hardware and cloud instance (sent by children in the stream handshake).

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_text::c::{c_str, fgets_chunks};
use netdata_agent_text::json::JsonWriter;
use netdata_agent_text::sanitize::rrd_string_sanitize;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemInfo {
    pub cloud_provider_type: Option<String>,
    pub cloud_instance_type: Option<String>,
    pub cloud_instance_region: Option<String>,
    pub host_os_name: Option<String>,
    pub host_os_id: Option<String>,
    pub host_os_id_like: Option<String>,
    pub host_os_version: Option<String>,
    pub host_os_version_id: Option<String>,
    pub host_os_detection: Option<String>,
    pub host_os_label_name: Option<String>,
    pub host_os_label_version: Option<String>,
    pub host_os_label_release: Option<String>,
    pub host_os_label_codename: Option<String>,
    pub host_os_label_edition: Option<String>,
    pub host_os_label_build: Option<String>,
    pub host_cores: Option<String>,
    pub host_cpu_freq: Option<String>,
    pub host_cpu_model: Option<String>,
    pub host_ram_total: Option<String>,
    pub host_disk_space: Option<String>,
    pub container_os_name: Option<String>,
    pub container_os_id: Option<String>,
    pub container_os_id_like: Option<String>,
    pub container_os_version: Option<String>,
    pub container_os_version_id: Option<String>,
    pub container_os_detection: Option<String>,
    pub kernel_name: Option<String>,
    pub kernel_version: Option<String>,
    pub architecture: Option<String>,
    pub virtualization: Option<String>,
    pub virt_detection: Option<String>,
    pub container: Option<String>,
    pub container_detection: Option<String>,
    pub is_k8s_node: Option<String>,
    pub hops: i16,
    pub ml_capable: bool,
    pub ml_enabled: bool,
    pub install_type: Option<String>,
    pub prebuilt_arch: Option<String>,
    pub prebuilt_dist: Option<String>,
    pub network_default_iface: Option<String>,
    pub network_default_iface_ip: Option<String>,
    pub network_default_iface_detection: Option<String>,
    pub mc_version: i32,
    pub hw_product_id: Option<String>,
    pub hw_product_name: Option<String>,
    pub hw_sys_vendor: Option<String>,
    pub hw_product_type: Option<String>,
}

/// A field as the JSON writers take it.
fn v(field: &Option<String>) -> Option<&[u8]> {
    field.as_deref().map(str::as_bytes)
}

impl SystemInfo {
    /// `rrdhost_system_info_set_by_name()`: `false` for a name it does not know (the caller logs it). Some known
    /// names are accepted and ignored; only the OS name is sanitized.
    pub fn set_by_name(&mut self, name: &str, value: &str) -> bool {
        match name {
            "NETDATA_PROTOCOL_VERSION"
            | "NETDATA_SYSTEM_CPU_VENDOR"
            | "NETDATA_SYSTEM_CPU_DETECTION"
            | "NETDATA_SYSTEM_RAM_DETECTION"
            | "NETDATA_SYSTEM_DISK_DETECTION"
            | "NETDATA_CONTAINER_IS_OFFICIAL_IMAGE" => {}
            "NETDATA_INSTANCE_CLOUD_TYPE" => self.cloud_provider_type = Some(value.to_string()),
            "NETDATA_INSTANCE_CLOUD_INSTANCE_TYPE" => {
                self.cloud_instance_type = Some(value.to_string())
            }
            "NETDATA_INSTANCE_CLOUD_INSTANCE_REGION" => {
                self.cloud_instance_region = Some(value.to_string())
            }
            "NETDATA_CONTAINER_OS_NAME" => self.container_os_name = Some(value.to_string()),
            "NETDATA_CONTAINER_OS_ID" => self.container_os_id = Some(value.to_string()),
            "NETDATA_CONTAINER_OS_ID_LIKE" => self.container_os_id_like = Some(value.to_string()),
            "NETDATA_CONTAINER_OS_VERSION" => self.container_os_version = Some(value.to_string()),
            "NETDATA_CONTAINER_OS_VERSION_ID" => {
                self.container_os_version_id = Some(value.to_string())
            }
            "NETDATA_CONTAINER_OS_DETECTION" => {
                self.container_os_detection = Some(value.to_string())
            }
            "NETDATA_HOST_OS_NAME" => {
                self.host_os_name = Some(
                    String::from_utf8_lossy(&rrd_string_sanitize(value.as_bytes())).into_owned(),
                )
            }
            "NETDATA_HOST_OS_ID" => self.host_os_id = Some(value.to_string()),
            "NETDATA_HOST_OS_ID_LIKE" => self.host_os_id_like = Some(value.to_string()),
            "NETDATA_HOST_OS_VERSION" => self.host_os_version = Some(value.to_string()),
            "NETDATA_HOST_OS_VERSION_ID" => self.host_os_version_id = Some(value.to_string()),
            "NETDATA_HOST_OS_DETECTION" => self.host_os_detection = Some(value.to_string()),
            "NETDATA_HOST_OS_LABEL_NAME" => self.host_os_label_name = Some(value.to_string()),
            "NETDATA_HOST_OS_LABEL_VERSION" => self.host_os_label_version = Some(value.to_string()),
            "NETDATA_HOST_OS_LABEL_RELEASE" => self.host_os_label_release = Some(value.to_string()),
            "NETDATA_HOST_OS_LABEL_CODENAME" => {
                self.host_os_label_codename = Some(value.to_string())
            }
            "NETDATA_HOST_OS_LABEL_EDITION" => self.host_os_label_edition = Some(value.to_string()),
            "NETDATA_HOST_OS_LABEL_BUILD" => self.host_os_label_build = Some(value.to_string()),
            "NETDATA_SYSTEM_KERNEL_NAME" => self.kernel_name = Some(value.to_string()),
            "NETDATA_SYSTEM_CPU_LOGICAL_CPU_COUNT" => self.host_cores = Some(value.to_string()),
            "NETDATA_SYSTEM_CPU_FREQ" => self.host_cpu_freq = Some(value.to_string()),
            "NETDATA_SYSTEM_CPU_MODEL" => self.host_cpu_model = Some(value.to_string()),
            "NETDATA_SYSTEM_TOTAL_RAM" => self.host_ram_total = Some(value.to_string()),
            "NETDATA_SYSTEM_TOTAL_DISK_SIZE" => self.host_disk_space = Some(value.to_string()),
            "NETDATA_SYSTEM_KERNEL_VERSION" => self.kernel_version = Some(value.to_string()),
            "NETDATA_SYSTEM_ARCHITECTURE" => self.architecture = Some(value.to_string()),
            "NETDATA_SYSTEM_VIRTUALIZATION" => self.virtualization = Some(value.to_string()),
            "NETDATA_SYSTEM_VIRT_DETECTION" => self.virt_detection = Some(value.to_string()),
            "NETDATA_SYSTEM_CONTAINER" => self.container = Some(value.to_string()),
            "NETDATA_SYSTEM_CONTAINER_DETECTION" => {
                self.container_detection = Some(value.to_string())
            }
            "NETDATA_HOST_IS_K8S_NODE" => self.is_k8s_node = Some(value.to_string()),
            "NETDATA_SYSTEM_DEFAULT_INTERFACE_NAME" => {
                self.network_default_iface = Some(value.to_string())
            }
            "NETDATA_SYSTEM_DEFAULT_INTERFACE_IP" => {
                self.network_default_iface_ip = Some(value.to_string())
            }
            "NETDATA_SYSTEM_DEFAULT_INTERFACE_DETECTION" => {
                self.network_default_iface_detection = Some(value.to_string())
            }
            _ => return false,
        }
        true
    }

    /// `rrdhost_system_info_to_json_v1()`: the members of `/api/v1/info`; missing ones print empty, the container,
    /// Kubernetes and cloud ones are left out.
    pub fn to_json_v1(&self, w: &mut JsonWriter) {
        w.member_add_string_or_empty("os_name", v(&self.host_os_name));
        w.member_add_string_or_empty("os_id", v(&self.host_os_id));
        w.member_add_string_or_empty("os_id_like", v(&self.host_os_id_like));
        w.member_add_string_or_empty("os_version", v(&self.host_os_version));
        w.member_add_string_or_empty("os_version_id", v(&self.host_os_version_id));
        w.member_add_string_or_empty("os_detection", v(&self.host_os_detection));
        w.member_add_string_or_empty("cores_total", v(&self.host_cores));
        w.member_add_string_or_empty("total_disk_space", v(&self.host_disk_space));
        w.member_add_string_or_empty("cpu_freq", v(&self.host_cpu_freq));
        w.member_add_string_or_empty("ram_total", v(&self.host_ram_total));
        w.member_add_string_or_omit("container_os_name", v(&self.container_os_name));
        w.member_add_string_or_omit("container_os_id", v(&self.container_os_id));
        w.member_add_string_or_omit("container_os_id_like", v(&self.container_os_id_like));
        w.member_add_string_or_omit("container_os_version", v(&self.container_os_version));
        w.member_add_string_or_omit("container_os_version_id", v(&self.container_os_version_id));
        w.member_add_string_or_omit("container_os_detection", v(&self.container_os_detection));
        w.member_add_string_or_omit("is_k8s_node", v(&self.is_k8s_node));
        w.member_add_string_or_empty("kernel_name", v(&self.kernel_name));
        w.member_add_string_or_empty("kernel_version", v(&self.kernel_version));
        w.member_add_string_or_empty("architecture", v(&self.architecture));
        w.member_add_string_or_empty("virtualization", v(&self.virtualization));
        w.member_add_string_or_empty("virt_detection", v(&self.virt_detection));
        w.member_add_string_or_empty("container", v(&self.container));
        w.member_add_string_or_empty("container_detection", v(&self.container_detection));
        w.member_add_string_or_omit("cloud_provider_type", v(&self.cloud_provider_type));
        w.member_add_string_or_omit("cloud_instance_type", v(&self.cloud_instance_type));
        w.member_add_string_or_omit("cloud_instance_region", v(&self.cloud_instance_region));
    }

    /// `rrdhost_system_info_to_rrdlabels()`: the `_*` labels of the fields that are set, in C's order.
    pub fn to_labels(&self, labels: &mut crate::labels::Labels) {
        let fields: [(&str, &Option<String>); 32] = [
            ("_cloud_provider_type", &self.cloud_provider_type),
            ("_cloud_instance_type", &self.cloud_instance_type),
            ("_cloud_instance_region", &self.cloud_instance_region),
            (
                "_os_name",
                if self.host_os_label_name.is_some() {
                    &self.host_os_label_name
                } else {
                    &self.host_os_name
                },
            ),
            ("_os_version", &self.host_os_version),
            ("_os_marketing_version", &self.host_os_label_version),
            ("_os_release", &self.host_os_label_release),
            ("_os_codename", &self.host_os_label_codename),
            ("_os_edition", &self.host_os_label_edition),
            ("_os_build", &self.host_os_label_build),
            ("_kernel_version", &self.kernel_version),
            ("_system_cores", &self.host_cores),
            ("_system_cpu_freq", &self.host_cpu_freq),
            ("_system_cpu_model", &self.host_cpu_model),
            ("_system_ram_total", &self.host_ram_total),
            ("_system_disk_space", &self.host_disk_space),
            ("_architecture", &self.architecture),
            ("_virtualization", &self.virtualization),
            ("_container", &self.container),
            ("_container_detection", &self.container_detection),
            ("_virt_detection", &self.virt_detection),
            ("_is_k8s_node", &self.is_k8s_node),
            ("_install_type", &self.install_type),
            ("_prebuilt_arch", &self.prebuilt_arch),
            ("_prebuilt_dist", &self.prebuilt_dist),
            ("_net_default_iface", &self.network_default_iface),
            ("_net_default_iface_ip", &self.network_default_iface_ip),
            (
                "_net_default_iface_detection",
                &self.network_default_iface_detection,
            ),
            ("_hw_product_id", &self.hw_product_id),
            ("_hw_product_name", &self.hw_product_name),
            ("_hw_sys_vendor", &self.hw_sys_vendor),
            ("_hw_product_type", &self.hw_product_type),
        ];
        for (name, value) in fields {
            if let Some(value) = value {
                labels.add(name.as_bytes(), value.as_bytes(), crate::labels::SRC_AUTO);
            }
        }
    }

    /// The parsing half of `rrdhost_system_info_detect()`: `system-info.sh`'s `NAME=value` lines, read as C's
    /// `fgets()` of 1022 bytes returns them, stored as `set_by_name()` stores them. Returns the pairs it accepted,
    /// which C exports to the environment; the lines it skips are logged as C logs them.
    pub fn apply_script_output(&mut self, stdout: &[u8]) -> Vec<(String, String)> {
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let mut accepted = Vec::new();
        for chunk in fgets_chunks(stdout, 1023) {
            let line = c_str(chunk);
            let Some(eq) = line.iter().position(|&c| c == b'=') else {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "SYSTEM INFO: Skipping malformed line from system-info.sh (no '=' found): '{}'",
                    text(line)
                );
                continue;
            };
            let name = text(&line[..eq]);
            let mut value = &line[eq + 1..];
            for end in *b"\n\r" {
                if let Some(at) = value.iter().position(|&c| c == end) {
                    value = &value[..at];
                }
            }
            let value = text(value);
            if name.is_empty() || value.is_empty() {
                nd_log!(
                    Source::Daemon,
                    Priority::Warning,
                    "SYSTEM INFO: Skipping empty name or value from system-info.sh: '{name}={value}'"
                );
            } else if self.set_by_name(&name, &value) {
                accepted.push((name, value));
            } else {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "SYSTEM INFO: Unexpected variable '{name}={value}'"
                );
            }
        }
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `system-info.sh`'s output on the development box (brief `knowledge/brief-localhost-identity.md` §4 in the
    /// status repository; the interface address redacted).
    const BOX_OUTPUT: &str = "NETDATA_CONTAINER_OS_NAME=none\n\
NETDATA_CONTAINER_OS_ID=none\n\
NETDATA_CONTAINER_OS_ID_LIKE=none\n\
NETDATA_CONTAINER_OS_VERSION=none\n\
NETDATA_CONTAINER_OS_VERSION_ID=none\n\
NETDATA_CONTAINER_OS_DETECTION=none\n\
NETDATA_CONTAINER_IS_OFFICIAL_IMAGE=false\n\
NETDATA_HOST_OS_NAME=Debian GNU/Linux\n\
NETDATA_HOST_OS_ID=debian\n\
NETDATA_HOST_OS_ID_LIKE=unknown\n\
NETDATA_HOST_OS_VERSION=13 (trixie)\n\
NETDATA_HOST_OS_VERSION_ID=13\n\
NETDATA_HOST_OS_DETECTION=/etc/os-release\n\
NETDATA_HOST_OS_LABEL_NAME=Debian GNU/Linux\n\
NETDATA_HOST_OS_LABEL_RELEASE=13\n\
NETDATA_HOST_OS_LABEL_CODENAME=trixie\n\
NETDATA_HOST_IS_K8S_NODE=false\n\
NETDATA_SYSTEM_KERNEL_NAME=Linux\n\
NETDATA_SYSTEM_KERNEL_VERSION=6.12.107+deb13-cloud-amd64\n\
NETDATA_SYSTEM_ARCHITECTURE=x86_64\n\
NETDATA_SYSTEM_VIRTUALIZATION=kvm\n\
NETDATA_SYSTEM_VIRT_DETECTION=systemd-detect-virt\n\
NETDATA_SYSTEM_CONTAINER=none\n\
NETDATA_SYSTEM_CONTAINER_DETECTION=systemd-detect-virt\n\
NETDATA_SYSTEM_CPU_LOGICAL_CPU_COUNT=16\n\
NETDATA_SYSTEM_CPU_VENDOR=AuthenticAMD\n\
NETDATA_SYSTEM_CPU_MODEL=QEMU Virtual CPU version 2.5+\n\
NETDATA_SYSTEM_CPU_FREQ=1999000000\n\
NETDATA_SYSTEM_CPU_DETECTION=lscpu procfs\n\
NETDATA_SYSTEM_TOTAL_RAM=33659879424\n\
NETDATA_SYSTEM_RAM_DETECTION=procfs\n\
NETDATA_SYSTEM_TOTAL_DISK_SIZE=429496729600\n\
NETDATA_SYSTEM_DISK_DETECTION=sysfs\n\
NETDATA_INSTANCE_CLOUD_TYPE=unknown\n\
NETDATA_INSTANCE_CLOUD_INSTANCE_TYPE=unknown\n\
NETDATA_INSTANCE_CLOUD_INSTANCE_REGION=unknown\n\
NETDATA_SYSTEM_DEFAULT_INTERFACE_NAME=eth0\n\
NETDATA_SYSTEM_DEFAULT_INTERFACE_IP=[PRIVATE_IP]\n\
NETDATA_SYSTEM_DEFAULT_INTERFACE_DETECTION=procfs\n\
";

    type Parsed = (SystemInfo, Vec<(String, String)>, Vec<(Priority, String)>);

    fn records(stdout: &[u8]) -> Parsed {
        let mut si = SystemInfo::default();
        let (accepted, records) = netdata_agent_log::capture(|| si.apply_script_output(stdout));
        let records = records
            .into_iter()
            .map(|r| (r.priority, r.message.unwrap_or_default()))
            .collect();
        (si, accepted, records)
    }

    #[test]
    fn the_box_output_is_stored_and_exported_whole() {
        let (si, accepted, logged) = records(BOX_OUTPUT.as_bytes());
        assert!(logged.is_empty(), "{logged:?}");
        let lines: Vec<(String, String)> = BOX_OUTPUT
            .lines()
            .map(|l| {
                let (n, v) = l.split_once('=').unwrap();
                (n.to_string(), v.to_string())
            })
            .collect();
        assert_eq!(accepted, lines);
        let some = |v: &str| Some(v.to_string());
        let expected = SystemInfo {
            cloud_provider_type: some("unknown"),
            cloud_instance_type: some("unknown"),
            cloud_instance_region: some("unknown"),
            host_os_name: some("Debian GNU/Linux"),
            host_os_id: some("debian"),
            host_os_id_like: some("unknown"),
            host_os_version: some("13 (trixie)"),
            host_os_version_id: some("13"),
            host_os_detection: some("/etc/os-release"),
            host_os_label_name: some("Debian GNU/Linux"),
            host_os_label_release: some("13"),
            host_os_label_codename: some("trixie"),
            host_cores: some("16"),
            host_cpu_freq: some("1999000000"),
            host_cpu_model: some("QEMU Virtual CPU version 2.5+"),
            host_ram_total: some("33659879424"),
            host_disk_space: some("429496729600"),
            container_os_name: some("none"),
            container_os_id: some("none"),
            container_os_id_like: some("none"),
            container_os_version: some("none"),
            container_os_version_id: some("none"),
            container_os_detection: some("none"),
            kernel_name: some("Linux"),
            kernel_version: some("6.12.107+deb13-cloud-amd64"),
            architecture: some("x86_64"),
            virtualization: some("kvm"),
            virt_detection: some("systemd-detect-virt"),
            container: some("none"),
            container_detection: some("systemd-detect-virt"),
            is_k8s_node: some("false"),
            network_default_iface: some("eth0"),
            network_default_iface_ip: some("[PRIVATE_IP]"),
            network_default_iface_detection: some("procfs"),
            ..SystemInfo::default()
        };
        assert_eq!(si, expected);
    }

    /// The crafted script of the brief (§1.4), whose records and values C was observed to produce.
    #[test]
    fn skipped_lines_are_logged_as_c() {
        let long = "e".repeat(1100);
        let stdout = format!(
            "NETDATA_SYSTEM_ARCHITECTURE=x86_64\nNETDATA_HOST_OS_NAME=Debian \"GNU\"/Linux, x\nno_equals_line\n\
             NETDATA_SYSTEM_CPU_MODEL=\n=value_without_name\nNETDATA_UNKNOWN_THING=1\nNETDATA_SYSTEM_KERNEL_NAME=Linux\r\n\
             NETDATA_SYSTEM_VIRTUALIZATION=a=b\nNETDATA_SYSTEM_CONTAINER=first\nNETDATA_SYSTEM_CONTAINER=second\n\
             NETDATA_HOST_OS_LABEL_EDITION={long}\nNETDATA_SYSTEM_TOTAL_RAM=123"
        );
        let (si, accepted, logged) = records(stdout.as_bytes());
        let malformed = |l: &str| {
            (
                Priority::Err,
                format!(
                    "SYSTEM INFO: Skipping malformed line from system-info.sh (no '=' found): '{l}'"
                ),
            )
        };
        let empty = |l: &str| {
            (
                Priority::Warning,
                format!("SYSTEM INFO: Skipping empty name or value from system-info.sh: '{l}'"),
            )
        };
        assert_eq!(
            logged,
            [
                malformed("no_equals_line\n"),
                empty("NETDATA_SYSTEM_CPU_MODEL="),
                empty("=value_without_name"),
                (
                    Priority::Err,
                    "SYSTEM INFO: Unexpected variable 'NETDATA_UNKNOWN_THING=1'".to_string()
                ),
                malformed(&format!("{}\n", "e".repeat(108))),
            ]
        );
        assert_eq!(si.host_os_name.as_deref(), Some("Debian 'GNU'/Linux, x"));
        assert_eq!(si.kernel_name.as_deref(), Some("Linux"));
        assert_eq!(si.virtualization.as_deref(), Some("a=b"));
        assert_eq!(si.container.as_deref(), Some("second"));
        assert_eq!(
            si.host_os_label_edition.as_deref(),
            Some("e".repeat(992).as_str())
        );
        assert_eq!(si.host_ram_total.as_deref(), Some("123"));
        assert_eq!(si.host_cpu_model, None);
        let names: Vec<&str> = accepted.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "NETDATA_SYSTEM_ARCHITECTURE",
                "NETDATA_HOST_OS_NAME",
                "NETDATA_SYSTEM_KERNEL_NAME",
                "NETDATA_SYSTEM_VIRTUALIZATION",
                "NETDATA_SYSTEM_CONTAINER",
                "NETDATA_SYSTEM_CONTAINER",
                "NETDATA_HOST_OS_LABEL_EDITION",
                "NETDATA_SYSTEM_TOTAL_RAM",
            ]
        );
        // the export carries the value as received, the struct the sanitized one
        assert_eq!(accepted[1].1, "Debian \"GNU\"/Linux, x");
    }

    #[test]
    fn known_names_are_stored() {
        let mut si = SystemInfo::default();
        assert!(si.set_by_name("NETDATA_SYSTEM_TOTAL_RAM", "1024"));
        assert!(si.set_by_name("NETDATA_SYSTEM_CPU_VENDOR", "x"));
        assert!(!si.set_by_name("tags", "x"));
        assert_eq!(si.host_ram_total.as_deref(), Some("1024"));
        assert!(si.set_by_name("NETDATA_HOST_OS_NAME", "Debian \"GNU\"/Linux"));
        assert_eq!(
            si.host_os_name.as_deref(),
            Some(String::from_utf8_lossy(&rrd_string_sanitize(b"Debian \"GNU\"/Linux")).as_ref())
        );
    }
}
