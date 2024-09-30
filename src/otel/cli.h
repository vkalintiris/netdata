#ifndef ND_OTEL_CLI_H
#define ND_OTEL_CLI_H

#include "CLI11.hpp"

#include <string>
#include <unordered_map>
#include <algorithm>
#include <cctype>

class NetdataConfig {
private:
    std::unordered_map<std::string, std::string> config_map;

    static std::string to_cli_param(const std::string &env_var)
    {
        std::string result = env_var;
        if (result.substr(0, 8) == "NETDATA_") {
            result = result.substr(8);
        }
        std::transform(result.begin(), result.end(), result.begin(), [](unsigned char c) { return std::tolower(c); });
        std::replace(result.begin(), result.end(), '_', '-');
        return result;
    }

public:
    NetdataConfig() = default;

    void add_options(CLI::App &app)
    {
        for (const auto &[key, value] : config_map) {
            app.add_option("--" + to_cli_param(key), config_map[key], "Set " + key);
        }
    }

    void set_defaults_from_env()
    {
        const char *EnvVars[] = {
            "NETDATA_CACHE_DIR",
            "NETDATA_CONFIG_DIR",
            "NETDATA_CONTAINER_IS_OFFICIAL_IMAGE",
            "NETDATA_CONTAINER_OS_DETECTION",
            "NETDATA_CONTAINER_OS_ID_LIKE",
            "NETDATA_CONTAINER_OS_ID",
            "NETDATA_CONTAINER_OS_NAME",
            "NETDATA_CONTAINER_OS_VERSION_ID",
            "NETDATA_CONTAINER_OS_VERSION",
            "NETDATA_DEBUG_FLAGS",
            "NETDATA_ERRORS_PER_PERIOD",
            "NETDATA_ERRORS_THROTTLE_PERIOD",
            "NETDATA_HOST_IS_K8S_NODE",
            "NETDATA_HOSTNAME",
            "NETDATA_HOST_OS_DETECTION",
            "NETDATA_HOST_OS_ID_LIKE",
            "NETDATA_HOST_OS_ID",
            "NETDATA_HOST_OS_NAME",
            "NETDATA_HOST_OS_VERSION",
            "NETDATA_HOST_OS_VERSION_ID",
            "NETDATA_HOST_PREFIX",
            "NETDATA_INSTANCE_CLOUD_INSTANCE_REGION",
            "NETDATA_INSTANCE_CLOUD_INSTANCE_TYPE",
            "NETDATA_INSTANCE_CLOUD_TYPE",
            "NETDATA_INTERNALS_EXTENDED_MONITORING",
            "NETDATA_INTERNALS_MONITORING",
            "NETDATA_INVOCATION_ID",
            "NETDATA_LIB_DIR",
            "NETDATA_LISTEN_PORT",
            "NETDATA_LOCK_DIR",
            "NETDATA_LOG_DIR",
            "NETDATA_LOG_FORMAT",
            "NETDATA_LOG_LEVEL",
            "NETDATA_LOG_METHOD",
            "NETDATA_PLUGINS_DIR",
            "NETDATA_REGISTRY_CLOUD_BASE_URL",
            "NETDATA_REGISTRY_HOSTNAME",
            "NETDATA_REGISTRY_UNIQUE_ID",
            "NETDATA_REGISTRY_URL",
            "NETDATA_STOCK_CONFIG_DIR",
            "NETDATA_SYSLOG_FACILITY",
            "NETDATA_SYSTEM_ARCHITECTURE",
            "NETDATA_SYSTEM_CONTAINER_DETECTION",
            "NETDATA_SYSTEM_CONTAINER",
            "NETDATA_SYSTEM_CPU_DETECTION",
            "NETDATA_SYSTEM_CPU_FREQ",
            "NETDATA_SYSTEM_CPU_LOGICAL_CPU_COUNT",
            "NETDATA_SYSTEM_CPU_MODEL",
            "NETDATA_SYSTEM_CPU_VENDOR",
            "NETDATA_SYSTEM_DISK_DETECTION",
            "NETDATA_SYSTEM_KERNEL_NAME",
            "NETDATA_SYSTEM_KERNEL_VERSION",
            "NETDATA_SYSTEM_RAM_DETECTION",
            "NETDATA_SYSTEM_TOTAL_DISK_SIZE",
            "NETDATA_SYSTEM_TOTAL_RAM",
            "NETDATA_SYSTEM_VIRT_DETECTION",
            "NETDATA_SYSTEM_VIRTUALIZATION",
            "NETDATA_UPDATE_EVERY",
            "NETDATA_USER_CONFIG_DIR",
            "NETDATA_USER_PLUGINS_DIRS",
            "NETDATA_VERSION",
            "NETDATA_WEB_DIR"};

        for (const auto &EnvVar : EnvVars) {
            const char *EnvPtr = std::getenv(EnvVar);
            config_map[EnvVar] = (EnvPtr != nullptr) ? EnvPtr : "";
        }
    }

    std::string get(const std::string &key) const
    {
        auto it = config_map.find(key);
        return (it != config_map.end()) ? it->second : "";
    }
};

#endif /* ND_OTEL_CLI_H */
