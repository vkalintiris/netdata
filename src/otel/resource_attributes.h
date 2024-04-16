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

class ResourceAttribute {
    /* TODO: handle enums, eg. flinkmetricsreceiver/metadata.yaml. */

    friend class ResourceAttributes;

private:
    static absl::StatusOr<ResourceAttribute> get(const YAML::Node &node)
    {
        if (!node["description"].IsScalar() || !node["enabled"].IsScalar() || !node["type"].IsScalar()) {
            return absl::InvalidArgumentError("Missing required fields or incorrect types in attribute node.");
        }

        std::string description = node["description"].as<std::string>();
        bool enabled = node["enabled"].as<bool>();
        std::string type = node["type"].as<std::string>();
        absl::variant<std::string, int> value;

        if (type == "string") {
            value = node["value"].IsDefined() ? node["value"].as<std::string>() : std::string("");
        } else if (type == "int") {
            value = node["value"].IsDefined() ? node["value"].as<int>() : 0;
        } else {
            return absl::InvalidArgumentError("Unsupported type specified in attribute node.");
        }

        return ResourceAttribute(description, enabled, value);
    }

private:
    ResourceAttribute() : Enabled(false)
    {
    }

    ResourceAttribute(const std::string &desc, bool en, const absl::variant<std::string, int> &val)
        : Description(desc), Enabled(en), Value(val)
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

    const absl::variant<std::string, int> &value() const
    {
        return Value;
    }

private:
    std::string Description;
    bool Enabled;
    absl::variant<std::string, int> Value;
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
            absl::visit([&OS](auto &&arg) { OS << arg << "\n"; }, attr.second.value());
        }
    }

private:
    std::map<std::string, ResourceAttribute> M;
};

#endif /* ND_RESOURCE_ATTRIBUTES_H */
