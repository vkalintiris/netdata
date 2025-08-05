use netdata_plugin_sdk::{PluginRuntime, Function, FunctionResult, PluginContext, FunctionContext};
use std::sync::Arc;
use tracing::info;

async fn hello_handler(plugin_ctx: Arc<PluginContext>, fn_ctx: FunctionContext) -> FunctionResult {
    info!("Handling hello function call from: {:?}", fn_ctx.source());
    
    // Access plugin statistics
    let stats = plugin_ctx.get_stats().await;
    info!("Plugin stats: {:?}", stats);
    
    let source = fn_ctx.get_parameter("source").unwrap_or("unknown");
    let message = format!(
        "Hello from hello-service SDK! You called function '{}' from source: '{}'.\n\
         This is implemented using the new SDK with enhanced runtime.\n\
         Transaction ID: {}\n\
         Plugin: {} | Total calls: {} | Active transactions: {}\n\
         Function elapsed: {:?}",
        fn_ctx.function_name(), 
        source,
        fn_ctx.transaction_id(),
        plugin_ctx.plugin_name(),
        stats.total_calls,
        stats.active_transactions,
        fn_ctx.elapsed()
    );
    
    FunctionResult::success(message)
}

async fn stats_handler(plugin_ctx: Arc<PluginContext>, fn_ctx: FunctionContext) -> FunctionResult {
    info!("Handling stats function call");
    
    // Get real plugin statistics from the context
    let plugin_stats = plugin_ctx.get_stats().await;
    let active_transactions = plugin_ctx.get_active_transactions().await;
    
    let stats = serde_json::json!({
        "plugin": plugin_ctx.plugin_name(),
        "version": "0.1.0",
        "function": fn_ctx.function_name(),
        "transaction": fn_ctx.transaction_id(),
        "parameters": fn_ctx.parameters(),
        "function_metadata": {
            "start_time": fn_ctx.metadata().start_time.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            "elapsed_ms": fn_ctx.elapsed().as_millis(),
            "timeout": fn_ctx.timeout(),
            "is_timed_out": fn_ctx.is_timed_out()
        },
        "statistics": {
            "total_calls": plugin_stats.total_calls,
            "successful_calls": plugin_stats.successful_calls,
            "failed_calls": plugin_stats.failed_calls,
            "timed_out_calls": plugin_stats.timed_out_calls,
            "active_transactions": plugin_stats.active_transactions,
            "active_transaction_details": active_transactions.iter().map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "function": t.function_name,
                    "elapsed_seconds": t.elapsed()
                })
            }).collect::<Vec<_>>()
        }
    });
    
    FunctionResult::json(stats)
}

async fn list_processes(plugin_ctx: Arc<PluginContext>, fn_ctx: FunctionContext) -> FunctionResult {
    info!("Handling list_processes function call");
    
    // Get information about other active functions in this plugin
    let active_transactions = plugin_ctx.get_active_transactions().await;
    let other_functions: Vec<String> = active_transactions
        .iter()
        .filter(|t| &t.id != fn_ctx.transaction_id())
        .map(|t| t.function_name.clone())
        .collect();
    
    // This is just a demo - in a real implementation you'd collect actual process data
    let processes = serde_json::json!({
        "processes": [
            {"pid": 1, "name": "init", "cpu": 0.1},
            {"pid": 100, "name": "hello-service", "cpu": 0.5},
            {"pid": 200, "name": "netdata", "cpu": 2.1}
        ],
        "total": 3,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "requested_by": fn_ctx.source().unwrap_or("unknown"),
        "plugin_context": {
            "concurrent_functions": other_functions,
            "plugin_name": plugin_ctx.plugin_name()
        },
        "function_timing": {
            "elapsed_ms": fn_ctx.elapsed().as_millis(),
            "timeout": fn_ctx.timeout()
        }
    });
    
    FunctionResult::json(processes)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("hello_service=info,netdata_plugin_sdk=info,netdata_plugin_runtime=info")
        .init();

    info!("Starting hello-service with enhanced SDK...");

    // Create the plugin runtime
    let runtime = PluginRuntime::new("hello-service-enhanced");

    // Register functions manually using the enhanced runtime
    runtime.register_function(Function {
        name: "hello".to_string(),
        help: "Returns a friendly hello message".to_string(),
        timeout: 10,
        tags: Some("greeting,demo".to_string()),
        access: Some(0),
        priority: Some(100),
        version: Some(1),
        global: false,
    }, hello_handler).await;

    runtime.register_function(Function {
        name: "stats".to_string(),
        help: "Returns plugin statistics".to_string(),
        timeout: 5,
        tags: Some("info,stats".to_string()),
        access: Some(0),
        priority: Some(100),
        version: Some(1),
        global: false,
    }, stats_handler).await;

    runtime.register_function(Function {
        name: "processes".to_string(),
        help: "Lists running processes (demo)".to_string(),
        timeout: 30,
        tags: Some("system,demo".to_string()),
        access: Some(0),
        priority: Some(100),
        version: Some(1),
        global: false,
    }, list_processes).await;

    info!("hello-service-enhanced configured, starting...");

    // Run the plugin
    runtime.run().await?;

    info!("hello-service-enhanced completed");
    Ok(())
}