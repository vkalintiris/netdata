//! `struct rrdhost_system_info`, ported from `src/database/rrdhost-system-info.{h,c}`: what a host reports about its
//! operating system, hardware and cloud instance (sent by children in the stream handshake).

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
