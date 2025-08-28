//! # Netdata Schema Generation Library
//!
//! This library provides functionality to generate Netdata-compatible JSON schemas
//! with UI annotations from Rust types annotated with schemars attributes.
//!
//! ## Basic Usage
//!
//! ```rust
//! use schemars::JsonSchema;
//! use serde::{Deserialize, Serialize};
//! use netdata_schema::NetdataSchema;
//!
//! #[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
//! #[schemars(extend("x-ui-flavour" = "tabs"))]
//! struct MyConfig {
//!     #[schemars(
//!         title = "Server URL",
//!         extend("x-ui-help" = "Enter the server URL"),
//!         extend("x-ui-placeholder" = "https://example.com")
//!     )]
//!     url: String,
//! }
//!
//! // NetdataSchema is automatically implemented for all JsonSchema types
//!
//! let netdata_schema = MyConfig::netdata_schema();
//! println!("{}", serde_json::to_string_pretty(&netdata_schema).unwrap());
//! ```

use schemars::transform::{Transform, transform_subschemas};
use schemars::{JsonSchema, Schema, SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value};

/// Transform that collects UI schema information from x-ui-* extensions
/// and removes them from the JSON schema, collecting them separately
#[derive(Default)]
struct CollectUISchema {
    ui_schema: Map<String, Value>,
    current_path: Vec<String>,
}

impl Transform for CollectUISchema {
    fn transform(&mut self, schema: &mut Schema) {
        let Some(obj) = schema.as_object_mut() else {
            return;
        };

        // Collect UI extensions from current schema
        let mut ui_props = Map::new();
        let mut keys_to_remove = Vec::new();

        for (key, value) in obj.iter() {
            if let Some(ui_key) = key.strip_prefix("x-ui-") {
                ui_props.insert(format!("ui:{}", ui_key), value.clone());
                keys_to_remove.push(key.clone());
            } else if key == "x-sensitive" && value == &Value::Bool(true) {
                ui_props.insert(
                    "ui:widget".to_string(),
                    Value::String("password".to_string()),
                );
                keys_to_remove.push(key.clone());
            }
        }

        // Remove the x-ui-* extensions from the JSON schema
        for key in keys_to_remove {
            obj.remove(&key);
        }

        // If we have UI properties, add them to the UI schema at the current path
        if !ui_props.is_empty() {
            let ui_path = if self.current_path.is_empty() {
                ".".to_string()
            } else {
                self.current_path.join(".")
            };

            if ui_path == "." {
                // Root level - merge into root UI schema
                for (key, value) in ui_props {
                    self.ui_schema.insert(key, value);
                }
            } else {
                self.ui_schema.insert(ui_path, Value::Object(ui_props));
            }
        }

        // Handle properties recursively
        if let Some(properties) = obj.get_mut("properties").and_then(|v| v.as_object_mut()) {
            for (prop_name, prop_schema) in properties.iter_mut() {
                if let Ok(schema_ref) = prop_schema.try_into() {
                    self.current_path.push(prop_name.clone());
                    self.transform(schema_ref);
                    self.current_path.pop();
                }
            }
        }

        // Handle definitions recursively
        if let Some(definitions) = obj.get_mut("definitions").and_then(|v| v.as_object_mut()) {
            for (def_name, def_schema) in definitions.iter_mut() {
                if let Ok(schema_ref) = def_schema.try_into() {
                    self.current_path.push(def_name.clone());
                    self.transform(schema_ref);
                    self.current_path.pop();
                }
            }
        }

        // Handle other subschemas
        transform_subschemas(self, schema);
    }
}

/// Configuration for Netdata schema generation
#[derive(Debug, Clone)]
struct NetdataSchemaConfig {
    /// Whether to include the full page UI option
    pub full_page: bool,
    /// JSON Schema settings to use
    pub schema_settings: SchemaSettings,
}

impl Default for NetdataSchemaConfig {
    fn default() -> Self {
        Self {
            full_page: true,
            schema_settings: SchemaSettings::draft07(),
        }
    }
}

/// Trait for types that can generate Netdata-compatible schemas
pub trait NetdataSchema: JsonSchema {
    /// Generate a Netdata-compatible schema with default configuration
    fn netdata_schema() -> serde_json::Value
    where
        Self: Sized,
    {
        generate_netdata_schema::<Self>()
    }
}

/// Generate a Netdata-compatible schema for the given type
pub fn generate_netdata_schema<T: JsonSchema>() -> serde_json::Value {
    let config = NetdataSchemaConfig::default();
    let generator = SchemaGenerator::new(config.schema_settings.clone());
    let mut schema = generator.into_root_schema_for::<T>();

    // Apply our UI schema collector transform
    let mut ui_collector = CollectUISchema::default();
    ui_collector.transform(&mut schema);

    // Create the UI schema from collected information
    let mut ui_schema = ui_collector.ui_schema;

    // Add default UI options
    if config.full_page {
        ui_schema.insert(
            "uiOptions".to_string(),
            serde_json::json!({
                "fullPage": true
            }),
        );
    }

    serde_json::json!({
        "jsonSchema": schema,
        "uiSchema": ui_schema
    })
}

/// Blanket implementation for all JsonSchema types
impl<T> NetdataSchema for T where T: JsonSchema {}
