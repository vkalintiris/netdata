use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;

/// Concatenates values from JSON object keys that match a given regex pattern
///
/// # Arguments
/// * `json_value` - Reference to a serde_json::Value (should be an object)
/// * `regex` - Precompiled regex pattern to match against keys
///
/// # Returns
/// * `String` - Concatenated values of matching keys, separated by spaces
///
/// # Examples
/// ```
/// use regex::Regex;
/// use serde_json::json;
///
/// let data = json!({
///     "metric.attributes.device": "/dev/dm-1",
///     "metric.attributes.mode": "rw",
///     "metric.name": "system.filesystem.utilization",
///     "scope.version": "0.128.0-dev"
/// });
///
/// let regex = Regex::new(r"^metric\.attributes\.").unwrap();
/// let result = concat_matching_values(&data, &regex);
/// // Result might be "/dev/dm-1 rw" (order may vary)
/// ```
pub fn concat_matching_values(json_value: &Value, regex: &Regex, sep: impl AsRef<str>) -> String {
    match json_value {
        Value::Object(map) => {
            // Use BTreeMap to ensure consistent ordering
            let mut matching_pairs: BTreeMap<&String, &Value> = BTreeMap::new();

            // Collect all key-value pairs where the key matches the regex
            for (key, value) in map {
                if regex.is_match(key) {
                    matching_pairs.insert(key, value);
                }
            }

            // Convert values to strings and concatenate
            matching_pairs
                .values()
                .filter_map(|value| value_to_string(value))
                .collect::<Vec<String>>()
                .join(sep.as_ref())
        }
        _ => String::new(), // Return empty string if not an object
    }
}

/// Converts a serde_json::Value to a String representation
fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some("null".to_string()),
        Value::Array(_) | Value::Object(_) => {
            panic!("Array and objects are not supported");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_concat_matching_values_basic() {
        let data = json!({
            "metric.attributes.device": "/dev/dm-1",
            "metric.attributes.mode": "rw",
            "metric.name": "system.filesystem.utilization",
            "scope.version": "0.128.0-dev"
        });

        let regex = Regex::new(r"^metric\.attributes\.").unwrap();
        let result = concat_matching_values(&data, &regex, ".");

        // Since we use BTreeMap, the order should be consistent
        assert!(result.contains("/dev/dm-1"));
        assert!(result.contains("rw"));
        assert!(!result.contains("system.filesystem.utilization"));
    }

    #[test]
    fn test_concat_matching_values_no_matches() {
        let data = json!({
            "foo": "bar",
            "baz": "qux"
        });

        let regex = Regex::new(r"^metric\.").unwrap();
        let result = concat_matching_values(&data, &regex, ".");

        assert_eq!(result, "");
    }

    #[test]
    fn test_concat_matching_values_different_types() {
        let data = json!({
            "test.string": "hello",
            "test.number": 42,
            "test.boolean": true,
            "test.null": null
        });

        let regex = Regex::new(r"^test\.").unwrap();
        let result = concat_matching_values(&data, &regex, ".");

        assert!(result.contains("hello"));
        assert!(result.contains("42"));
        assert!(result.contains("true"));
        assert!(result.contains("null"));
    }

    #[test]
    fn test_concat_matching_values_not_object() {
        let data = json!("not an object");
        let regex = Regex::new(r".*").unwrap();
        let result = concat_matching_values(&data, &regex, ".");

        assert_eq!(result, "");
    }

    #[test]
    fn test_with_your_example_data() {
        let data = json!({
            "metric.attributes.device": "/dev/dm-1",
            "metric.attributes.mode": "rw",
            "metric.attributes.mountpoint": "/",
            "metric.attributes.type": "ext4",
            "metric.description": "Fraction of filesystem bytes used.",
            "metric.hash": "859ba79d4f9b9c4c",
            "metric.name": "system.filesystem.utilization",
            "metric.start_time_unix_nano": 1751111202000000000u64,
            "metric.time_unix_nano": 1751172558666165888u64,
            "metric.type": "gauge",
            "metric.unit": "1",
            "metric.value": 0.73162931276602,
            "scope.name": "github.com/open-telemetry/opentelemetry-collector-contrib/receiver/hostmetricsreceiver/internal/scraper/filesystemscraper",
            "scope.version": "0.128.0-dev"
        });

        // Get all metric attributes
        let regex = Regex::new(r"^metric\.attributes\.").unwrap();
        let result = concat_matching_values(&data, &regex, ".");

        assert!(result.contains("/dev/dm-1"));
        assert!(result.contains("rw"));
        assert!(result.contains("/"));
        assert!(result.contains("ext4"));
    }
}
