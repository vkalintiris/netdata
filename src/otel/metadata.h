#ifndef NETDATA_OTEL_METADATA_H
#define NETDATA_OTEL_METADATA_H

#include <set>
#include <absl/types/optional.h>
#include <absl/status/status.h>
#include <absl/status/statusor.h>
#include <yaml-cpp/yaml.h>

namespace otel
{
namespace config
{
    class Metric {
    public:
        Metric(const YAML::Node &Node)
        {
            if (Node["dimensions_attribute"]) {
                DimensionsAttribute = Node["dimensions_attribute"].as<std::string>();
            }

            if (Node["instance_attributes"]) {
                const auto &V = Node["instance_attributes"].as<std::vector<std::string> >();
                InstanceAttributes.insert(V.begin(), V.end());
            }
        }

        const std::string *getDimensionsAttribute() const
        {
            return &DimensionsAttribute;
        }

        const std::set<std::string> *getInstanceAttributes() const
        {
            return &InstanceAttributes;
        }

    private:
        std::string DimensionsAttribute;
        std::set<std::string> InstanceAttributes;
    };

    class Scope {
    public:
        Scope(const YAML::Node &node)
        {
            for (const auto &M : node["metrics"]) {
                Metrics.emplace(M.first.as<std::string>(), Metric(M.second));
            }
        }

        const Metric *getMetric(const std::string &Name) const
        {
            auto It = Metrics.find(Name);
            return (It != Metrics.end()) ? &(It->second) : nullptr;
        }

    private:
        std::unordered_map<std::string, Metric> Metrics;
    };

    class Config {
    public:
        Config(const std::string &Path)
        {
            YAML::Node config = YAML::LoadFile(Path);

            for (const auto &scope : config["scopes"]) {
                Scopes.emplace(scope.first.as<std::string>(), Scope(scope.second));
            }
        }

        const Scope *getScope(const std::string &Name) const
        {
            auto It = Scopes.find(Name);
            return (It != Scopes.end()) ? &(It->second) : nullptr;
        }

        const Metric *getMetric(const std::string &ScopeName, const std::string &MetricName) const
        {
            const Scope *S = getScope(ScopeName);
            if (!S)
                return nullptr;

            return S->getMetric(MetricName);
        }

        const std::string *getDimensionsAttribute(const std::string &ScopeName, const std::string &MetricName) const
        {
            const Metric *M = getMetric(ScopeName, MetricName);
            if (!M)
                return nullptr;

            return M->getDimensionsAttribute();
        }

        const std::set<std::string> *
        getInstanceAttribute(const std::string &ScopeName, const std::string &MetricName) const
        {
            const Metric *M = getMetric(ScopeName, MetricName);
            if (!M)
                return nullptr;

            return M->getInstanceAttributes();
        }

    private:
        std::unordered_map<std::string, Scope> Scopes;
    };

} // namespace config
} // namespace otel

#endif /* NETDATA_OTEL_METADATA_H */
