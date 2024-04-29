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

enum class ValueType { String, Bool, Integer, StringEnum, None };

class ResourceAttribute {
    friend class ResourceAttributes;

public:
    // static const std::string KeyId;

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
        : Type(ValueType::StringEnum), Description(Description), Enabled(Enabled), Discriminants(Discriminants)
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

    const absl::optional<std::vector<std::string> > &discriminants() const
    {
        assert(Type == ValueType::StringEnum);
        return Discriminants;
    }

    void dump(std::ostream &OS) const
    {
        OS << "Description: " << Description << "\n";
        OS << "Enabled: " << Enabled << "\n";

        switch (Type) {
            case ValueType::String:
                OS << "Type: string";
                break;
            case ValueType::Bool:
                OS << "Type: bool";
                break;
            case ValueType::Integer:
                OS << "Type: integer";
                break;
            case ValueType::StringEnum:
                OS << "Type: string (enum)";
                break;
            case ValueType::None:
                OS << "None";
                break;
            default:
                break;
        }
        OS << "\n";

        if (Type == ValueType::StringEnum) {
            OS << "Enum: ";
            for (size_t Idx = 0; Idx != Discriminants->size(); Idx++) {
                OS << Discriminants->at(Idx);

                if (Idx < Discriminants->size() - 1) {
                    OS << " | ";
                }
            }
        }
        OS << "\n";
    }

    friend std::ostream &operator<<(std::ostream &os, const ResourceAttribute &attr);

private:
    ValueType Type;
    std::string Description;
    bool Enabled;
    absl::optional<std::vector<std::string> > Discriminants;
};

class MetricAttribute {
    friend class MetricAttributes;

private:
    static absl::StatusOr<MetricAttribute> get(const YAML::Node &node)
    {
        if (!node["description"].IsScalar() || !node["type"].IsScalar()) {
            return absl::InvalidArgumentError("Missing required fields or incorrect types in resource attribute node.");
        }

        std::string Description = node["description"].as<std::string>();

        std::string TypeStr = node["type"].as<std::string>();

        ValueType Type = ValueType::None;
        if (TypeStr == "string") {
            Type = ValueType::String;
        } else if (TypeStr == "int") {
            Type = ValueType::Integer;
        } else if (TypeStr == "bool") {
            Type = ValueType::Bool;
        } else {
            return absl::InvalidArgumentError("Unsupported type specified in resource attribute node.");
        }

        std::string NameOverride;
        if (node["name_override"]) {
            if (!node["name_override"].IsScalar())
                return absl::InvalidArgumentError("name_override is not a scalar");

            NameOverride = node["name_override"].as<std::string>();
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

            return MetricAttribute(Description, NameOverride, Discriminants);
        }

        return MetricAttribute(Description, Type, NameOverride);
    }

private:
    MetricAttribute() : Type(ValueType::None)
    {
    }

    MetricAttribute(const std::string &Description, ValueType Type) : Type(Type), Description(Description)
    {
    }

    MetricAttribute(const std::string &Description, ValueType Type, const std::string &NameOverride)
        : Type(Type), Description(Description), NameOverride(NameOverride)
    {
    }

    MetricAttribute(const std::string &Description, const std::vector<std::string> &Discriminants)
        : Type(ValueType::StringEnum), Description(Description), Discriminants(Discriminants)
    {
        (void)Type;
    }

    MetricAttribute(
        const std::string &Description,
        const std::string &NameOverride,
        const std::vector<std::string> &Discriminants)
        : Type(ValueType::StringEnum), Description(Description), NameOverride(NameOverride),
          Discriminants(Discriminants)
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

    const std::string &nameOverride() const
    {
        return NameOverride;
    }

    const absl::optional<std::vector<std::string> > &discriminants() const
    {
        assert(Type == ValueType::StringEnum);
        return Discriminants;
    }

    void dump(std::ostream &OS) const
    {
        OS << "Description: " << Description << "\n";
        OS << "NameOverride: " << NameOverride << "\n";

        switch (Type) {
            case ValueType::String:
                OS << "Type: string";
                break;
            case ValueType::Bool:
                OS << "Type: bool";
                break;
            case ValueType::Integer:
                OS << "Type: integer";
                break;
            case ValueType::StringEnum:
                OS << "Type: string (enum)";
                break;
            case ValueType::None:
                OS << "None";
                break;
            default:
                break;
        }
        OS << "\n";

        if (Type == ValueType::StringEnum) {
            OS << "Enum: ";
            for (size_t Idx = 0; Idx != Discriminants->size(); Idx++) {
                OS << Discriminants->at(Idx);

                if (Idx < Discriminants->size() - 1) {
                    OS << " | ";
                }
            }
        }
        OS << "\n";
    }

    friend std::ostream &operator<<(std::ostream &os, const MetricAttribute &attr);

private:
    ValueType Type;
    std::string Description;
    std::string NameOverride;
    absl::optional<std::vector<std::string> > Discriminants;
};

class MetricAttributes {
public:
    static absl::StatusOr<MetricAttributes> get(const YAML::Node &node)
    {
        if (!node["attributes"]) {
            return absl::NotFoundError("The key 'attributes' does not exist in the YAML file.");
        }

        MetricAttributes attributes;
        for (const auto &attribute : node["attributes"]) {
            const auto attributeResult = MetricAttribute::get(attribute.second);
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

    void dump(std::ostream &OS) const
    {
        OS << "[Attribute]\n";

        for (const auto &P : M) {
            OS << "Key: " << P.first << "\n";
            OS << P.second << "\n";
        }
    }

private:
    std::map<std::string, MetricAttribute> M;
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

    void dump(std::ostream &OS) const
    {
        OS << "[ResourceAttribute]\n";

        for (const auto &P : M) {
            OS << "Key: " << P.first << "\n";
            OS << P.second << "\n";
        }
    }

private:
    std::map<std::string, ResourceAttribute> M;
};

#endif /* ND_RESOURCE_ATTRIBUTES_H */
