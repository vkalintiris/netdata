use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use bumpalo::Bump;
use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{any_value::Value, AnyValue, KeyValue},
    logs::v1::LogRecord,
    resource::v1::Resource,
};
use serde_json::{Map as JsonMap, Value as JsonValue};

pub struct PreparedLogEntry<'a> {
    pub items: &'a [&'a [u8]],
    pub source_timestamp_usec: Option<u64>,
    pub sort_key: u64,
}

struct EntryBuilder<'a> {
    bump: &'a Bump,
    items: bumpalo::collections::Vec<'a, &'a [u8]>,
    otlp_map: Option<JsonMap<String, JsonValue>>,
}

impl<'a> EntryBuilder<'a> {
    fn new(bump: &'a Bump, store_otlp_json: bool) -> Self {
        Self {
            bump,
            items: bumpalo::collections::Vec::new_in(bump),
            otlp_map: if store_otlp_json {
                Some(JsonMap::new())
            } else {
                None
            },
        }
    }

    fn push_str(&mut self, key: &str, value: &str) {
        let s = bumpalo::format!(in self.bump, "{}={}", key, value);
        self.items.push(s.into_bytes().into_bump_slice());
        if let Some(map) = &mut self.otlp_map {
            map.insert(key.to_string(), JsonValue::String(value.to_string()));
        }
    }

    fn push_u64(&mut self, key: &str, value: u64) {
        let s = bumpalo::format!(in self.bump, "{}={}", key, value);
        self.items.push(s.into_bytes().into_bump_slice());
        if let Some(map) = &mut self.otlp_map {
            map.insert(key.to_string(), JsonValue::Number(value.into()));
        }
    }

    fn push_i64(&mut self, key: &str, value: i64) {
        let s = bumpalo::format!(in self.bump, "{}={}", key, value);
        self.items.push(s.into_bytes().into_bump_slice());
        if let Some(map) = &mut self.otlp_map {
            map.insert(key.to_string(), JsonValue::Number(value.into()));
        }
    }

    fn push_i32(&mut self, key: &str, value: i32) {
        let s = bumpalo::format!(in self.bump, "{}={}", key, value);
        self.items.push(s.into_bytes().into_bump_slice());
        if let Some(map) = &mut self.otlp_map {
            map.insert(key.to_string(), JsonValue::Number(value.into()));
        }
    }

    fn push_f64(&mut self, key: &str, value: f64) {
        let s = bumpalo::format!(in self.bump, "{}={}", key, value);
        self.items.push(s.into_bytes().into_bump_slice());
        if let Some(map) = &mut self.otlp_map {
            if let Some(n) = serde_json::Number::from_f64(value) {
                map.insert(key.to_string(), JsonValue::Number(n));
            } else {
                map.insert(key.to_string(), JsonValue::Null);
            }
        }
    }

    fn push_bool(&mut self, key: &str, value: bool) {
        let s = bumpalo::format!(in self.bump, "{}={}", key, value);
        self.items.push(s.into_bytes().into_bump_slice());
        if let Some(map) = &mut self.otlp_map {
            map.insert(key.to_string(), JsonValue::Bool(value));
        }
    }

    fn push_null(&mut self, key: &str) {
        let s = bumpalo::format!(in self.bump, "{}=null", key);
        self.items.push(s.into_bytes().into_bump_slice());
        if let Some(map) = &mut self.otlp_map {
            map.insert(key.to_string(), JsonValue::Null);
        }
    }

    fn finish(mut self) -> &'a [&'a [u8]] {
        if let Some(map) = self.otlp_map.take() {
            if let Ok(json_str) = serde_json::to_string(&JsonValue::Object(map)) {
                let s = bumpalo::format!(in self.bump, "OTLP_JSON={}", json_str);
                let bytes: &[u8] = s.into_bytes().into_bump_slice();
                // Prepend OTLP_JSON as the first item
                let len = self.items.len();
                self.items.push(&[]); // placeholder to grow
                let slice = self.items.as_mut_slice();
                slice.copy_within(..len, 1);
                slice[0] = bytes;
            }
        }
        self.items.into_bump_slice()
    }
}

fn write_resource_items(builder: &mut EntryBuilder<'_>, resource: &Resource) {
    write_key_value_items(builder, "resource.attributes", &resource.attributes);
}

fn write_scope_items(
    builder: &mut EntryBuilder<'_>,
    scope: &opentelemetry_proto::tonic::common::v1::InstrumentationScope,
) {
    if !scope.name.is_empty() {
        builder.push_str("scope.name", &scope.name);
    }
    if !scope.version.is_empty() {
        builder.push_str("scope.version", &scope.version);
    }
    if !scope.attributes.is_empty() {
        write_key_value_items(builder, "scope.attributes", &scope.attributes);
    }
}

fn write_log_record_items(builder: &mut EntryBuilder<'_>, rec: &LogRecord) {
    builder.push_u64("log.time_unix_nano", rec.time_unix_nano);
    builder.push_u64("log.observed_time_unix_nano", rec.observed_time_unix_nano);
    builder.push_i32("log.severity_number", rec.severity_number);
    builder.push_str("log.severity_text", &rec.severity_text);

    // Body handling
    if let Some(body) = &rec.body {
        write_body(builder, body);
    }

    builder.push_str("log.event_name", &rec.event_name);

    // Log attributes
    if !rec.attributes.is_empty() {
        write_key_value_items(builder, "log.attributes", &rec.attributes);
    }

    builder.push_u64(
        "log.dropped_attributes_count",
        rec.dropped_attributes_count as u64,
    );
    builder.push_u64("log.flags", rec.flags as u64);
}

fn write_body(builder: &mut EntryBuilder<'_>, body: &AnyValue) {
    match &body.value {
        Some(Value::StringValue(s)) => {
            // Try JSON parse (cold path) - only flatten if it's an object
            if let Ok(JsonValue::Object(map)) = serde_json::from_str(s) {
                flatten_json_object(builder, "log.body", &map);
            } else {
                builder.push_str("log.body", s);
            }
        }
        Some(Value::KvlistValue(kvl)) => {
            write_key_value_items(builder, "log.body", &kvl.values);
        }
        Some(Value::IntValue(i)) => builder.push_i64("log.body", *i),
        Some(Value::DoubleValue(d)) => builder.push_f64("log.body", *d),
        Some(Value::BoolValue(b)) => builder.push_bool("log.body", *b),
        Some(Value::ArrayValue(arr)) => {
            for val in &arr.values {
                write_any_value(builder, "log.body", val);
            }
        }
        Some(Value::BytesValue(bytes)) => {
            let encoded = BASE64.encode(bytes);
            builder.push_str("log.body", &encoded);
        }
        None => {}
    }
}

fn write_key_value_items(builder: &mut EntryBuilder<'_>, prefix: &str, kvl: &[KeyValue]) {
    for kv in kvl {
        let key = bumpalo::format!(in builder.bump, "{}.{}", prefix, kv.key);
        let key_str: &str = key.into_bump_str();

        match kv.value.as_ref().and_then(|v| v.value.as_ref()) {
            Some(Value::StringValue(s)) => builder.push_str(key_str, s),
            Some(Value::IntValue(i)) => builder.push_i64(key_str, *i),
            Some(Value::DoubleValue(d)) => builder.push_f64(key_str, *d),
            Some(Value::BoolValue(b)) => builder.push_bool(key_str, *b),
            Some(Value::KvlistValue(kvl)) => {
                write_key_value_items(builder, key_str, &kvl.values);
            }
            Some(Value::ArrayValue(arr)) => {
                for val in &arr.values {
                    write_any_value(builder, key_str, val);
                }
            }
            Some(Value::BytesValue(bytes)) => {
                let encoded = BASE64.encode(bytes);
                builder.push_str(key_str, &encoded);
            }
            None => builder.push_null(key_str),
        }
    }
}

fn write_any_value(builder: &mut EntryBuilder<'_>, key: &str, val: &AnyValue) {
    match &val.value {
        Some(Value::StringValue(s)) => builder.push_str(key, s),
        Some(Value::IntValue(i)) => builder.push_i64(key, *i),
        Some(Value::DoubleValue(d)) => builder.push_f64(key, *d),
        Some(Value::BoolValue(b)) => builder.push_bool(key, *b),
        Some(Value::KvlistValue(kvl)) => {
            write_key_value_items(builder, key, &kvl.values);
        }
        Some(Value::ArrayValue(arr)) => {
            for val in &arr.values {
                write_any_value(builder, key, val);
            }
        }
        Some(Value::BytesValue(bytes)) => {
            let encoded = BASE64.encode(bytes);
            builder.push_str(key, &encoded);
        }
        None => builder.push_null(key),
    }
}

fn flatten_json_object(builder: &mut EntryBuilder<'_>, prefix: &str, map: &JsonMap<String, JsonValue>) {
    for (key, value) in map {
        let full_key = bumpalo::format!(in builder.bump, "{}.{}", prefix, key);
        let full_key_str: &str = full_key.into_bump_str();

        match value {
            JsonValue::Object(nested) => flatten_json_object(builder, full_key_str, nested),
            JsonValue::String(s) => builder.push_str(full_key_str, s),
            JsonValue::Number(n) => {
                let s = bumpalo::format!(in builder.bump, "{}={}", full_key_str, n);
                builder.items.push(s.into_bytes().into_bump_slice());
                if let Some(map) = &mut builder.otlp_map {
                    map.insert(full_key_str.to_string(), JsonValue::Number(n.clone()));
                }
            }
            JsonValue::Bool(b) => builder.push_bool(full_key_str, *b),
            JsonValue::Array(arr) => {
                for val in arr {
                    write_json_value(builder, full_key_str, val);
                }
            }
            JsonValue::Null => builder.push_null(full_key_str),
        }
    }
}

fn write_json_value(builder: &mut EntryBuilder<'_>, key: &str, val: &JsonValue) {
    match val {
        JsonValue::String(s) => builder.push_str(key, s),
        JsonValue::Number(n) => {
            let s = bumpalo::format!(in builder.bump, "{}={}", key, n);
            builder.items.push(s.into_bytes().into_bump_slice());
            if let Some(map) = &mut builder.otlp_map {
                map.insert(key.to_string(), JsonValue::Number(n.clone()));
            }
        }
        JsonValue::Bool(b) => builder.push_bool(key, *b),
        JsonValue::Object(nested) => flatten_json_object(builder, key, nested),
        JsonValue::Array(arr) => {
            for val in arr {
                write_json_value(builder, key, val);
            }
        }
        JsonValue::Null => builder.push_null(key),
    }
}

#[tracing::instrument(skip_all)]
pub fn prepare_log_entries<'a>(
    bump: &'a Bump,
    request: &ExportLogsServiceRequest,
    store_otlp_json: bool,
) -> bumpalo::collections::Vec<'a, PreparedLogEntry<'a>> {
    let mut entries = bumpalo::collections::Vec::new_in(bump);

    for resource_logs in &request.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for log_record in &scope_logs.log_records {
                let mut builder = EntryBuilder::new(bump, store_otlp_json);

                // Resource attributes
                if let Some(resource) = resource_logs.resource.as_ref() {
                    write_resource_items(&mut builder, resource);
                }

                // Resource schema URL
                if !resource_logs.schema_url.is_empty() {
                    builder.push_str("resource.schema_url", &resource_logs.schema_url);
                }

                // Scope information
                if let Some(scope) = scope_logs.scope.as_ref() {
                    write_scope_items(&mut builder, scope);
                }

                // Scope schema URL
                if !scope_logs.schema_url.is_empty() {
                    builder.push_str("scope.schema_url", &scope_logs.schema_url);
                }

                // Log record fields
                write_log_record_items(&mut builder, log_record);

                // Compute timestamp for sorting and source_realtime_usec
                let time_nano = if log_record.time_unix_nano != 0 {
                    Some(log_record.time_unix_nano)
                } else {
                    None
                };
                let observed_nano = if log_record.observed_time_unix_nano != 0 {
                    Some(log_record.observed_time_unix_nano)
                } else {
                    None
                };

                let sort_key = time_nano.or(observed_nano).unwrap_or(0);
                let source_timestamp_usec = time_nano.or(observed_nano).map(|n| n / 1000);

                let items = builder.finish();

                entries.push(PreparedLogEntry {
                    items,
                    source_timestamp_usec,
                    sort_key,
                });
            }
        }
    }

    entries
}
