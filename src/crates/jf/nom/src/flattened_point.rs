use serde_json::{Map as JsonMap, Value as JsonValue};
use std::hash::{Hash, Hasher};

use crate::regex_cache::RegexCache;

#[derive(Default, Debug)]
pub struct FlattenedPoint {
    pub attributes: JsonMap<String, JsonValue>,

    pub nd_instance_name: String,
    pub nd_dimension_name: String,

    pub metric_name: String,
    pub metric_unit: String,
    pub metric_type: String,

    pub metric_time_unix_nano: u64,
    pub metric_value: f64,
}

impl FlattenedPoint {
    pub fn new(mut json_map: JsonMap<String, JsonValue>, regex_cache: &RegexCache) -> Option<Self> {
        let metric_name = match json_map.remove("metric.name").unwrap() {
            JsonValue::String(s) => s,
            _ => return None,
        };
        let metric_unit = match json_map.remove("metric.unit").unwrap() {
            JsonValue::String(s) => s,
            _ => return None,
        };
        let metric_type = match json_map.remove("metric.type").unwrap() {
            JsonValue::String(s) => s,
            _ => return None,
        };
        let metric_time_unix_nano = match json_map.remove("metric.time_unix_nano").unwrap() {
            JsonValue::Number(n) => n.as_u64()?,
            _ => return None,
        };
        let metric_value = match json_map.remove("metric.value").unwrap() {
            JsonValue::Number(n) => n.as_f64()?,
            _ => return None,
        };

        let nd_chart_instance = match json_map.remove("metric.attributes._nd_chart_instance") {
            Some(JsonValue::String(s)) => Some(regex_cache.get(&s).unwrap()),
            _ => return None,
        };

        let nd_dimension_key = match json_map.remove("metric.attributes._nd_dimension") {
            Some(JsonValue::String(s)) => Some(s),
            _ => return None,
        };

        let nd_instance_name = {
            let Some(pattern) = nd_chart_instance.as_ref() else {
                return None;
            };

            let mut matched_values = Vec::new();
            for (key, value) in &json_map {
                if pattern.is_match(key) {
                    let value_str = match value {
                        JsonValue::String(s) => s.clone(),
                        JsonValue::Number(n) => n.to_string(),
                        JsonValue::Bool(b) => b.to_string(),
                        JsonValue::Null => "null".to_string(),
                        _ => serde_json::to_string(value).unwrap_or_default(),
                    };
                    matched_values.push(value_str);
                }
            }

            let suffix = matched_values.join(".");
            Some(format!("{}.{}", metric_name, suffix))
        }
        .unwrap_or(metric_name.clone());

        let nd_dimension_name = {
            let Some(key) = nd_dimension_key.as_ref() else {
                return None;
            };

            let Some(value) = json_map.remove(key) else {
                return None;
            };

            let s = match value {
                JsonValue::String(s) => s.clone(),
                JsonValue::Number(n) => n.to_string(),
                JsonValue::Bool(b) => b.to_string(),
                JsonValue::Null => "null".to_string(),
                _ => unimplemented!(),
            };

            Some(s)
        }
        .unwrap_or(String::from("value"));

        Some(Self {
            attributes: json_map,
            nd_instance_name,
            nd_dimension_name,
            metric_name,
            metric_unit,
            metric_type,
            metric_time_unix_nano,
            metric_value,
        })
    }

    fn metric_description(&self) -> &str {
        match self.attributes.get("metric.description") {
            Some(JsonValue::String(s)) => s,
            Some(_) | None => "",
        }
    }
}

impl Hash for FlattenedPoint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.attributes.hash(state);

        self.metric_name.hash(state);
        self.metric_unit.hash(state);
        self.metric_type.hash(state);
    }
}
