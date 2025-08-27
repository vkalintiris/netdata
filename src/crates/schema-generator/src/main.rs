use netdata_schema::NetdataSchema;
use schemars::{JsonSchema, SchemaGenerator, generate::SchemaSettings};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
struct MyCredentials {
    #[schemars(
        title = "Username",
        description = "Username for authentication",
        example = &"admin",
        extend("x-ui-help" = "Enter your login username"),
        extend("x-ui-placeholder" = "Enter username...")
    )]
    username: String,

    #[schemars(
        title = "Password",
        description = "Password for authentication",
        extend("x-ui-widget" = "password"),
        extend("x-ui-help" = "Enter your login password"),
        extend("x-ui-placeholder" = "Enter password..."),
        extend("x-sensitive" = true)
    )]
    password: String,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[schemars(
    title = "Demo Plugin Configuration",
    description = "Configuration for the demo plugin",
    extend("x-ui-flavour" = "tabs"),
    extend("x-ui-options" = {
        "tabs": [
            {
                "title": "Connection",
                "fields": ["url", "port"]
            },
            {
                "title": "Authentication", 
                "fields": ["credentials"]
            },
            {
                "title": "Files",
                "fields": ["files"]
            }
        ]
    })
)]
struct MyConfig {
    #[schemars(
        title = "Server URL",
        description = "The base URL for the server endpoint",
        example = "https://api.example.com",
        url,
        extend("x-ui-help" = "Full URL including protocol (http:// or https://)"),
        extend("x-ui-placeholder" = "https://example.com")
    )]
    url: String,

    #[schemars(
        title = "Port Number",
        description = "TCP port for server connection", 
        range(min = 1, max = 65535),
        example = 8080,
        extend("x-ui-help" = "Standard TCP port number (1-65535)"),
        extend("x-ui-placeholder" = "8080")
    )]
    port: u16,

    #[schemars(
        title = "Credentials",
        description = "Optional authentication credentials",
        extend("x-ui-help" = "Leave empty for anonymous access")
    )]
    credentials: Option<MyCredentials>,

    #[schemars(
        title = "Configuration Files",
        description = "List of configuration files to load",
        extend("x-ui-help" = "Add one file path per line. Supports glob patterns like *.conf"),
        extend("x-ui-placeholder" = "/etc/myapp/*.conf"),
        extend("x-ui-widget" = "textarea")
    )]
    files: Vec<String>,
}

impl MyConfig {
    pub fn default_example() -> Self {
        Self {
            url: "https://api.example.com".to_string(),
            port: 8080,
            credentials: Some(MyCredentials {
                username: "admin".to_string(),
                password: "secret123".to_string(),
            }),
            files: vec![
                "/etc/myapp/main.conf".to_string(),
                "/etc/myapp/plugins/*.conf".to_string(),
                "/var/lib/myapp/config.yaml".to_string(),
            ],
        }
    }
}

// Implementation moved to netdata-schema library crate

fn main() {
    // Generate standard JSON schema (with UI extensions)
    let settings = SchemaSettings::draft07();
    let generator = SchemaGenerator::new(settings);
    let schema = generator.into_root_schema_for::<MyConfig>();

    println!("=== Standard JSON Schema (with UI extensions) ===");
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());

    // Generate clean JSON schema (without UI extensions)
    let clean_schema = MyConfig::clean_json_schema();
    println!("\n=== Clean JSON Schema (no UI extensions) ===");
    println!("{}", serde_json::to_string_pretty(&clean_schema).unwrap());

    // Generate UI schema only
    let ui_schema = MyConfig::ui_schema();
    println!("\n=== UI Schema Only ===");
    println!("{}", serde_json::to_string_pretty(&ui_schema).unwrap());

    // Generate Netdata format with UI schema (using the trait)
    let netdata_schema = MyConfig::netdata_schema();
    println!("\n=== Netdata Format (JSON Schema + UI Schema) ===");
    println!("{}", serde_json::to_string_pretty(&netdata_schema).unwrap());

    // Generate example config instance
    let example_config = MyConfig::default_example();
    println!("\n=== Example Configuration Instance ===");
    println!("{}", serde_json::to_string_pretty(&example_config).unwrap());
}
