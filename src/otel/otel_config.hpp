#ifndef NETDATA_OTEL_CONFIG_HPP
#define NETDATA_OTEL_CONFIG_HPP

#include <yaml-cpp/yaml.h>

namespace otel
{
    class MetricConfig {
    public:
        MetricConfig(const YAML::Node &Node)
        {
            if (Node["dimensions_attribute"]) {
                DimensionsAttribute = Node["dimensions_attribute"].as<std::string>();
            }

            if (Node["instance_attributes"]) {
                InstanceAttributes = Node["instance_attributes"].as<std::vector<std::string> >();
            }
        }

        const std::string *getDimensionsAttribute() const
        {
            return &DimensionsAttribute;
        }

        const std::vector<std::string> *getInstanceAttributes() const
        {
            return &InstanceAttributes;
        }

    private:
        std::string DimensionsAttribute;
        std::vector<std::string> InstanceAttributes;
    };

    class ScopeConfig {
    public:
        ScopeConfig(const YAML::Node &node)
        {
            for (const auto &M : node["metrics"]) {
                Metrics.emplace(M.first.as<std::string>(), MetricConfig(M.second));
            }
        }

        const MetricConfig *getMetric(const std::string &Name) const
        {
            auto It = Metrics.find(Name);
            return (It != Metrics.end()) ? &(It->second) : nullptr;
        }

    private:
        std::unordered_map<std::string, MetricConfig> Metrics;
    };

    class Config {
    public:
        Config(const std::string &Path)
        {
            YAML::Node config = YAML::LoadFile(Path);

            for (const auto &scope : config["scopes"]) {
                Scopes.emplace(scope.first.as<std::string>(), ScopeConfig(scope.second));
            }
        }

        const ScopeConfig *getScope(const std::string &Name) const
        {
            auto It = Scopes.find(Name);
            return (It != Scopes.end()) ? &(It->second) : nullptr;
        }

        const MetricConfig *getMetric(const std::string &ScopeName, const std::string &MetricName) const
        {
            const ScopeConfig *S = getScope(ScopeName);
            if (!S)
                return nullptr;

            return S->getMetric(MetricName);
        }

        const std::string *getDimensionsAttribute(const std::string &ScopeName, const std::string &MetricName) const
        {
            const MetricConfig *M = getMetric(ScopeName, MetricName);
            if (!M)
                return nullptr;

            return M->getDimensionsAttribute();
        }

        const std::vector<std::string> *
        getInstanceAttribute(const std::string &ScopeName, const std::string &MetricName) const
        {
            const MetricConfig *M = getMetric(ScopeName, MetricName);
            if (!M)
                return nullptr;

            return M->getInstanceAttributes();
        }

    private:
        std::unordered_map<std::string, ScopeConfig> Scopes;
    };

} // namespace otel

#endif /* NETDATA_OTEL_CONFIG_HPP */
