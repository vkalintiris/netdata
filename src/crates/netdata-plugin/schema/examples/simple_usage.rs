use netdata_schema::NetdataSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[schemars(
    title = "Web Server Configuration",
    description = "Configuration for a simple web server",
    extend("x-ui-flavour" = "tabs"),
    extend("x-ui-options" = {
        "tabs": [
            {
                "title": "Server Settings",
                "fields": ["host", "port", "workers"]
            },
            {
                "title": "Security",
                "fields": ["enable_tls", "tls_cert_path", "api_key"]
            }
        ]
    })
)]
struct WebServerConfig {
    #[schemars(
        title = "Host Address",
        description = "The IP address to bind the server to",
        example = "0.0.0.0",
        extend("x-ui-help" = "Use 0.0.0.0 to bind to all interfaces"),
        extend("x-ui-placeholder" = "127.0.0.1")
    )]
    host: String,

    #[schemars(
        title = "Port",
        description = "TCP port number",
        range(min = 1, max = 65535),
        example = 8080,
        extend("x-ui-help" = "Choose an available port number"),
        extend("x-ui-placeholder" = "8080")
    )]
    port: u16,

    #[schemars(
        title = "Worker Threads",
        description = "Number of worker threads",
        range(min = 1, max = 32),
        extend("x-ui-help" = "Usually should match the number of CPU cores")
    )]
    #[serde(default = "default_workers")]
    workers: Option<u8>,

    #[schemars(
        title = "Enable TLS",
        description = "Enable HTTPS/TLS encryption",
        extend("x-ui-widget" = "checkbox")
    )]
    #[serde(default)]
    enable_tls: bool,

    #[schemars(
        title = "TLS Certificate Path",
        description = "Path to the TLS certificate file",
        extend("x-ui-help" = "Required when TLS is enabled"),
        extend("x-ui-placeholder" = "/etc/ssl/certs/server.crt")
    )]
    tls_cert_path: Option<String>,

    #[schemars(
        title = "API Key",
        description = "Secret key for API authentication",
        extend("x-ui-widget" = "password"),
        extend("x-ui-help" = "Keep this secret! Used for API authentication"),
        extend("x-sensitive" = true)
    )]
    api_key: String,
}

fn default_workers() -> Option<u8> {
    Some(4)
}

fn main() {
    println!("=== Clean JSON Schema ===");
    let clean_schema = WebServerConfig::clean_json_schema();
    println!("{}", serde_json::to_string_pretty(&clean_schema).unwrap());

    println!("\n=== UI Schema Only ===");
    let ui_schema = WebServerConfig::ui_schema();
    println!("{}", serde_json::to_string_pretty(&ui_schema).unwrap());

    println!("\n=== Complete Netdata Schema ===");
    let netdata_schema = WebServerConfig::netdata_schema();
    println!("{}", serde_json::to_string_pretty(&netdata_schema).unwrap());
}
