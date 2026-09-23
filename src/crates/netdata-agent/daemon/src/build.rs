//! Build-time constants the C agent gets from CMake (`config.h`): install paths, the service user and the version.
//! The build sets them through environment variables; the defaults are those of a standard install.

macro_rules! build_env {
    ($name:ident, $var:literal, $default:literal) => {
        pub const $name: &str = match option_env!($var) {
            Some(value) => value,
            None => $default,
        };
    };
}

build_env!(CONFIG_DIR, "NETDATA_CONFIG_DIR", "/etc/netdata");
build_env!(
    LIBCONFIG_DIR,
    "NETDATA_LIBCONFIG_DIR",
    "/usr/lib/netdata/conf.d"
);
build_env!(
    STOCK_DATA_DIR,
    "NETDATA_STOCK_DATA_DIR",
    "/usr/share/netdata"
);
build_env!(LOG_DIR, "NETDATA_LOG_DIR", "/var/log/netdata");
build_env!(
    PLUGINS_DIR,
    "NETDATA_PLUGINS_DIR",
    "/usr/libexec/netdata/plugins.d"
);
build_env!(WEB_DIR, "NETDATA_WEB_DIR", "/usr/share/netdata/web");
build_env!(CACHE_DIR, "NETDATA_CACHE_DIR", "/var/cache/netdata");
build_env!(VARLIB_DIR, "NETDATA_VARLIB_DIR", "/var/lib/netdata");
build_env!(NETDATA_USER, "NETDATA_USER", "netdata");
// The agent reports the version of the C release it replaces: parents, Cloud and dashboards see it.
build_env!(
    NETDATA_VERSION,
    "NETDATA_VERSION",
    "v2.11.0-458-g1e97a0fc9e"
);

/// `CONFIG_FILENAME`.
pub const CONFIG_FILENAME: &str = "netdata.conf";
