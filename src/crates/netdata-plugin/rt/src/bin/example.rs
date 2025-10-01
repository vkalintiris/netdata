use async_trait::async_trait;
use netdata_plugin_protocol::{FunctionDeclaration, FunctionResult};
use rt::{FunctionContext, FunctionHandler, PluginRuntime};
use std::sync::Arc;
use tracing::info;

struct HelloWorldHandler;

#[async_trait]
impl FunctionHandler for HelloWorldHandler {
    async fn handle(&self, ctx: Arc<FunctionContext>) -> FunctionResult {
        info!("Hello world function called with transaction: {}", ctx.function_call.transaction);

        FunctionResult {
            transaction: ctx.function_call.transaction.clone(),
            status: 200,
            expires: 0,
            format: "text/plain".to_string(),
            payload: "Hello, World!".as_bytes().to_vec(),
        }
    }

    async fn progress(&self, _ctx: FunctionContext) -> FunctionResult {
        // Not implemented for this simple example
        FunctionResult {
            transaction: String::new(),
            status: 501,
            expires: 0,
            format: "text/plain".to_string(),
            payload: "Progress not supported".as_bytes().to_vec(),
        }
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration::new("hello_world", "A simple hello world function")
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,netdata_plugin_channels=debug".to_string()),
        )
        .init();

    info!("Starting example plugin");

    let mut runtime = PluginRuntime::new("example");
    runtime.register_handler(Arc::new(HelloWorldHandler));
    runtime.run().await?;

    Ok(())
}
