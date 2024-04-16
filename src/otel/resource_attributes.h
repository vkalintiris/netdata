#ifndef ND_RESOURCE_ATTRIBUTES_H
#define ND_RESOURCE_ATTRIBUTES_H

#include <iostream>
#include <string>
#include <map>

#include <absl/types/optional.h>
#include <absl/types/variant.h>
#include <absl/status/status.h>
#include <absl/status/statusor.h>
#include <absl/strings/str_cat.h>

#include <yaml-cpp/yaml.h>

enum class ValueType { String, Bool, Integer, EnumList, None };

class ResourceAttribute {
    friend class ResourceAttributes;

private:
    static absl::StatusOr<ResourceAttribute> get(const YAML::Node &node)
    {
        if (!node["description"].IsScalar() || !node["enabled"].IsScalar() || !node["type"].IsScalar()) {
            return absl::InvalidArgumentError("Missing required fields or incorrect types in resource attribute node.");
        }

        std::string Description = node["description"].as<std::string>();
        bool Enabled = node["enabled"].as<bool>();

        std::string Type = node["type"].as<std::string>();
        if (Type == "string") {
            return ResourceAttribute(Description, Enabled, ValueType::String);
        } else if (Type == "int") {
            return ResourceAttribute(Description, Enabled, ValueType::Integer);
        } else {
            return absl::InvalidArgumentError("Unsupported type specified in resource attribute node.");
        }

        if (node["enum"]) {
            if (!node["enum"].IsSequence())
                return absl::InvalidArgumentError("Enum does not contain a sequence");

            std::vector<std::string> Discriminants;
            for (const YAML::Node &nd : node["enum"]) {
                if (nd.IsScalar()) {
                    Discriminants.push_back(nd.as<std::string>());
                } else {
                    return absl::InvalidArgumentError("Enum values must be scalar strings.");
                }
            }

            return ResourceAttribute(Description, Enabled, Discriminants);
        }

        return absl::InvalidArgumentError("Malformed resource attribute node");
    }

private:
    ResourceAttribute() : Type(ValueType::None), Enabled(false)
    {
    }

    ResourceAttribute(const std::string &Description, bool Enabled, ValueType Type)
        : Type(Type), Description(Description), Enabled(Enabled)
    {
    }

    ResourceAttribute(const std::string &Description, bool Enabled, const std::vector<std::string> &Discriminants)
        : Type(ValueType::EnumList), Description(Description), Enabled(Enabled), Discriminants(Discriminants)
    {
    }

public:
    const std::string &description() const
    {
        return Description;
    }

    bool enabled() const
    {
        return Enabled;
    }

    ValueType type() const
    {
        assert(Type == ValueType::Integer || Type == ValueType::String);
        return Type;
    }

    const std::optional<std::vector<std::string> > &discriminants() const
    {
        assert(Type == ValueType::EnumList);
        return Discriminants;
    }

private:
    ValueType Type;
    std::string Description;
    bool Enabled;
    absl::optional<std::vector<std::string> > Discriminants;
};

class MetricAttribute {
private:
    static absl::StatusOr<MetricAttribute> get(const YAML::Node &node)
    {
        if (!node["description"].IsScalar() || !node["enabled"].IsScalar() || !node["type"].IsScalar()) {
            return absl::InvalidArgumentError("Missing required fields or incorrect types in resource attribute node.");
        }

        std::string Description = node["description"].as<std::string>();

        std::string Type = node["type"].as<std::string>();
        if (Type == "string") {
            return MetricAttribute(Description, ValueType::String);
        } else if (Type == "int") {
            return MetricAttribute(Description, ValueType::Integer);
        } else if (Type == "bool") {
            return MetricAttribute(Description, ValueType::Bool);
        } else {
            return absl::InvalidArgumentError("Unsupported type specified in resource attribute node.");
        }

        if (node["enum"]) {
            if (!node["enum"].IsSequence())
                return absl::InvalidArgumentError("Enum does not contain a sequence");

            std::vector<std::string> Discriminants;
            for (const YAML::Node &nd : node["enum"]) {
                if (nd.IsScalar()) {
                    Discriminants.push_back(nd.as<std::string>());
                } else {
                    return absl::InvalidArgumentError("Enum values must be scalar strings.");
                }
            }

            return MetricAttribute(Description, Discriminants);
        }

        return absl::InvalidArgumentError("Malformed resource attribute node");
    }

private:
    MetricAttribute() : Type(ValueType::None)
    {
    }

    MetricAttribute(const std::string &Description, ValueType Type)
        : Type(Type), Description(Description)
    {
    }

    MetricAttribute(const std::string &Description, const std::string &NameOverride)
        : Type(ValueType::String), Description(Description), NameOverride(NameOverride)
    {
    }

    MetricAttribute(const std::string &Description, const std::vector<std::string> &Discriminants)
        : Type(ValueType::EnumList), Description(Description), Discriminants(Discriminants)
    {
    }

public:
    const std::string &description() const
    {
        return Description;
    }

    ValueType type() const
    {
        assert(Type == ValueType::Integer || Type == ValueType::String);
        return Type;
    }

    const std::string &nameOverride() const {
        assert(Type == ValueType::String);
        return NameOverride;
    }

    const std::optional<std::vector<std::string> > &discriminants() const
    {
        assert(Type == ValueType::EnumList);
        return Discriminants;
    }

private:
    ValueType Type;
    std::string Description;
    std::string NameOverride;
    absl::optional<std::vector<std::string> > Discriminants;
};

class ResourceAttributes {
public:
    static absl::StatusOr<ResourceAttributes> get(const YAML::Node &node)
    {
        if (!node["resource_attributes"]) {
            return absl::NotFoundError("The key 'resource_attributes' does not exist in the YAML file.");
        }

        ResourceAttributes attributes;
        for (const auto &attribute : node["resource_attributes"]) {
            const auto attributeResult = ResourceAttribute::get(attribute.second);
            if (!attributeResult.ok()) {
                return absl::Status(
                    absl::StatusCode::kInvalidArgument,
                    absl::StrCat("Failed to parse attribute: ", attributeResult.status().message()));
            }

            const auto &P = std::make_pair(attribute.first.as<std::string>(), *attributeResult);
            attributes.M.insert(P);
        }

        return attributes;
    }

    void printAttributes(std::ostream &OS) const
    {
        for (const auto &attr : M) {
            OS << "Attribute: " << attr.first << "\nDescription: " << attr.second.description()
               << "\nEnabled: " << (attr.second.enabled() ? "Yes" : "No") << "\nValue: ";
        }
    }

private:
    std::map<std::string, ResourceAttribute> M;
};

#endif /* ND_RESOURCE_ATTRIBUTES_H */
