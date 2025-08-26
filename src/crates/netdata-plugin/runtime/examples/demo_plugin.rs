#![allow(dead_code, unused_imports)]

//! Example plugin demonstrating the netdata-plugin-runtime usage
//!
//! This example shows how to:
//! - Create a plugin runtime
//! - Register multiple functions with different behaviors
//! - Handle plugin and function contexts
//! - Manage transactions and cancellations
//! - Access plugin statistics

use netdata_plugin_runtime::{
    ConfigDeclarable, ConfigDeclaration, DynCfgCmds, DynCfgSourceType, DynCfgStatus, DynCfgType,
    FunctionContext, FunctionDeclaration, FunctionResult, HttpAccess, PluginContext, PluginRuntime,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};

/// A simple greeting function
async fn hello_function(plugin_ctx: PluginContext, fn_ctx: FunctionContext) -> FunctionResult {
    info!(
        "hello_function called: transaction={}, source={:?}",
        fn_ctx.transaction_id(),
        fn_ctx.source()
    );

    // Get plugin statistics
    let stats = plugin_ctx.get_stats().await;

    let response = format!(
        "Hello from {}!\n\
         Transaction: {}\n\
         Function: {}\n\
         Source: {}\n\
         Plugin Stats:\n\
         - Total calls: {}\n\
         - Successful: {}\n\
         - Failed: {}\n\
         - Active: {}\n\
         - Elapsed: {:?}",
        plugin_ctx.plugin_name(),
        fn_ctx.transaction_id(),
        fn_ctx.function_name(),
        fn_ctx.source().unwrap_or("unknown"),
        stats.total_calls,
        stats.successful_calls,
        stats.failed_calls,
        stats.active_transactions,
        fn_ctx.elapsed(),
    );

    FunctionResult {
        transaction: fn_ctx.transaction_id().clone(),
        status: 200,
        format: "text/plain".to_string(),
        expires: 0,
        payload: response.into_bytes(),
    }
}

/// A function that processes data from the payload
async fn process_data(_plugin_ctx: PluginContext, fn_ctx: FunctionContext) -> FunctionResult {
    info!(
        "process_data called: transaction={}, has_payload={}",
        fn_ctx.transaction_id(),
        fn_ctx.payload().is_some()
    );

    // Check if we have payload data
    let response = if let Some(payload) = fn_ctx.payload() {
        match String::from_utf8(payload.to_vec()) {
            Ok(data) => {
                // Simulate some processing
                let processed = data.to_uppercase();
                format!(
                    "Processed data successfully!\n\
                     Original length: {}\n\
                     Processed: {}",
                    data.len(),
                    processed
                )
            }
            Err(e) => format!("Error decoding payload: {}", e),
        }
    } else {
        "No payload data provided".to_string()
    };

    FunctionResult {
        transaction: fn_ctx.transaction_id().clone(),
        status: 200,
        format: "text/plain".to_string(),
        expires: 0,
        payload: response.into_bytes(),
    }
}

/// A slow function that can be cancelled
async fn slow_function(plugin_ctx: PluginContext, fn_ctx: FunctionContext) -> FunctionResult {
    info!(
        "slow_function called: transaction={}, timeout={}s",
        fn_ctx.transaction_id(),
        fn_ctx.timeout()
    );

    // Simulate a slow operation with cancellation checks
    for i in 0..10 {
        // Check for cancellation via both methods:
        // 1. Check if transaction was cancelled (explicit cancellation)
        // 2. Check if function context is cancelled (shutdown or timeout)
        if plugin_ctx
            .is_transaction_cancelled(fn_ctx.transaction_id())
            .await
            || fn_ctx.is_cancelled()
        {
            info!(
                "Transaction {} was cancelled after {} iterations (shutdown: {})",
                fn_ctx.transaction_id(),
                i,
                fn_ctx.is_cancelled()
            );
            return FunctionResult {
                transaction: fn_ctx.transaction_id().clone(),
                status: 499, // Client Closed Request
                format: "text/plain".to_string(),
                expires: 0,
                payload: format!("Operation cancelled after {} iterations", i).into_bytes(),
            };
        }

        // Simulate work - can also use tokio::select! for immediate cancellation
        tokio::select! {
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                // Work completed
            }
            _ = fn_ctx.cancellation_token().cancelled() => {
                info!("Detected cancellation during sleep");
                return FunctionResult {
                    transaction: fn_ctx.transaction_id().clone(),
                    status: 499,
                    format: "text/plain".to_string(),
                    expires: 0,
                    payload: format!("Operation cancelled during iteration {}", i).into_bytes(),
                };
            }
        }
    }

    FunctionResult {
        transaction: fn_ctx.transaction_id().clone(),
        status: 200,
        format: "text/plain".to_string(),
        expires: 0,
        payload: format!(
            "Slow operation completed successfully after {:?}",
            fn_ctx.elapsed()
        )
        .into_bytes(),
    }
}

/// Get active transactions
async fn get_transactions(plugin_ctx: PluginContext, fn_ctx: FunctionContext) -> FunctionResult {
    let transactions = plugin_ctx.get_active_transactions().await;

    let mut response = format!("Active transactions: {}\n\n", transactions.len());
    for tx in transactions {
        response.push_str(&format!(
            "- Transaction: {}\n  Function: {}\n  Source: {:?}\n  Elapsed: {:?}\n  Cancelled: {}\n\n",
            tx.id,
            tx.function_name,
            tx.source,
            tx.elapsed(),
            tx.cancelled
        ));
    }

    FunctionResult {
        transaction: fn_ctx.transaction_id().clone(),
        status: 200,
        format: "text/plain".to_string(),
        expires: 0,
        payload: response.into_bytes(),
    }
}

/// Reset plugin statistics
async fn reset_stats(plugin_ctx: PluginContext, fn_ctx: FunctionContext) -> FunctionResult {
    plugin_ctx.reset_stats().await;

    FunctionResult {
        transaction: fn_ctx.transaction_id().clone(),
        status: 200,
        format: "text/plain".to_string(),
        expires: 0,
        payload: b"Statistics reset successfully".to_vec(),
    }
}

pub async fn register_functions(runtime: &PluginRuntime) -> Result<(), Box<dyn std::error::Error>> {
    // Register functions
    runtime
        .register_function(
            FunctionDeclaration {
                name: "hello".to_string(),
                help: "Returns a friendly greeting with plugin statistics".to_string(),
                timeout: 10,
                tags: Some("greeting,info".to_string()),
                access: Some(HttpAccess::from_u32(0)),
                priority: Some(100),
                version: Some(1),
                global: false,
            },
            hello_function,
        )
        .await?;

    runtime
        .register_function(
            FunctionDeclaration {
                name: "process".to_string(),
                help: "Processes data from the payload".to_string(),
                timeout: 30,
                tags: Some("data,processing".to_string()),
                access: Some(HttpAccess::from_u32(0)),
                priority: Some(90),
                version: Some(1),
                global: false,
            },
            process_data,
        )
        .await?;

    runtime
        .register_function(
            FunctionDeclaration {
                name: "slow".to_string(),
                help: "A slow operation that can be cancelled".to_string(),
                timeout: 60,
                tags: Some("slow,cancellable".to_string()),
                access: Some(HttpAccess::from_u32(0)),
                priority: Some(50),
                version: Some(1),
                global: false,
            },
            slow_function,
        )
        .await?;

    runtime
        .register_function(
            FunctionDeclaration {
                name: "transactions".to_string(),
                help: "Get list of active transactions".to_string(),
                timeout: 5,
                tags: Some("admin,debug".to_string()),
                access: Some(HttpAccess::from_u32(0)),
                priority: Some(200),
                version: Some(1),
                global: false,
            },
            get_transactions,
        )
        .await?;

    runtime
        .register_function(
            FunctionDeclaration {
                name: "reset-stats".to_string(),
                help: "Reset plugin statistics".to_string(),
                timeout: 5,
                tags: Some("admin".to_string()),
                access: Some(HttpAccess::from_u32(0)),
                priority: Some(200),
                version: Some(1),
                global: false,
            },
            reset_stats,
        )
        .await?;

    Ok(())
}

pub async fn register_configs(runtime: &PluginRuntime) -> Result<(), Box<dyn std::error::Error>> {
    let settings = SchemaSettings::draft07();
    let generator = SchemaGenerator::new(settings);
    let schema = generator.into_root_schema_for::<MyConfig>();
    eprintln!("{}", serde_json::to_string_pretty(&schema).unwrap());

    runtime.register_config::<MyConfig>().await.unwrap();
    Ok(())
}

use schemars::{JsonSchema, SchemaGenerator, generate::SchemaSettings, schema_for};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
struct MyCredentials {
    username: String,
    password: String,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
struct MyConfig {
    url: String,
    port: u16,
    credentials: Option<MyCredentials>,
}

impl ConfigDeclarable for MyConfig {
    fn config_declaration() -> ConfigDeclaration {
        ConfigDeclaration {
            id: String::from("demo_plugin:my_config"),
            status: DynCfgStatus::None,
            type_: DynCfgType::Single,
            path: String::from("demo_plugin/my_config"),
            source_type: DynCfgSourceType::Stock,
            source: String::from("Whatever source help info"),
            cmds: DynCfgCmds::SCHEMA,
            view_access: HttpAccess::empty(),
            edit_access: HttpAccess::empty(),
        }
    }
}

impl MyConfig {
    pub fn new(url: &str, port: u16) -> Self {
        Self {
            url: String::from(url),
            port,
            credentials: Some(MyCredentials {
                username: String::from("vk"),
                password: String::from("123456"),
            }),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing with tokio-console support
    if std::env::var("TOKIO_CONSOLE").is_ok() {
        console_subscriber::init();
    } else {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(
                std::env::var("RUST_LOG")
                    .unwrap_or_else(|_| "demo_plugin=info,netdata_plugin_runtime=info".to_string()),
            )
            .init();
    }

    info!("Starting demo plugin...");

    // Create the plugin runtime
    let runtime = PluginRuntime::new("demo-plugin");

    register_functions(&runtime).await?;

    register_configs(&runtime).await?;

    info!("All functions registered, starting runtime...");

    // Run the plugin
    match runtime.run().await {
        Ok(()) => {
            info!("Plugin runtime completed successfully");
        }
        Err(e) => {
            error!("Plugin runtime error: {}", e);
            std::process::exit(1);
        }
    }

    info!("Exiting demo plugin");
    std::process::exit(0)
}
