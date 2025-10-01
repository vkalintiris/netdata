use async_trait::async_trait;
use netdata_plugin_protocol::{FunctionDeclaration, FunctionResult};
use rt::{ControlMessage, FunctionContext, FunctionHandler, PluginRuntime};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

struct HelloFastHandler;

#[async_trait]
impl FunctionHandler for HelloFastHandler {
    async fn handle(&self, ctx: Arc<FunctionContext>) -> FunctionResult {
        info!(
            "Fast function called with transaction: {}",
            ctx.function_call.transaction
        );

        FunctionResult {
            transaction: ctx.function_call.transaction.clone(),
            status: 200,
            expires: 0,
            format: "text/plain".to_string(),
            payload: "Fast response!".as_bytes().to_vec(),
        }
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration::new("hello_fast", "A fast function that responds immediately")
    }
}

struct HelloSlowHandler;

#[async_trait]
impl FunctionHandler for HelloSlowHandler {
    async fn handle(&self, ctx: Arc<FunctionContext>) -> FunctionResult {
        info!(
            "Slow function called with transaction: {}",
            ctx.function_call.transaction
        );

        let transaction = ctx.function_call.transaction.clone();
        let mut control_rx = ctx.control_rx.lock().await;

        // Simulate slow work - 10 seconds total, check for cancellation every 500ms
        for i in 0..20 {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => {
                    info!("Slow function progress: {}%", (i + 1) * 5);
                }
                Some(msg) = control_rx.recv() => {
                    match msg {
                        ControlMessage::Progress => {
                            info!("Called progress");
                        }
                    }
                }
                _ = ctx.cancellation_token.cancelled() => {
                    warn!("Slow function cancelled at {}%", (i + 1) * 5);
                    return FunctionResult {
                        transaction,
                        status: 499,
                        expires: 0,
                        format: "text/plain".to_string(),
                        payload: format!("Cancelled after {:.1} seconds", (i + 1) as f32 * 0.5).as_bytes().to_vec(),
                    };
                }
            }
        }

        info!("Slow function completed");
        FunctionResult {
            transaction,
            status: 200,
            expires: 0,
            format: "text/plain".to_string(),
            payload: "Slow work completed successfully!".as_bytes().to_vec(),
        }
    }

    fn declaration(&self) -> FunctionDeclaration {
        FunctionDeclaration::new(
            "hello_slow",
            "A slow function that takes 10 seconds and respects cancellation",
        )
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,netdata_plugin_channels=debug".to_string()),
        )
        .init();

    info!("Starting example plugin");

    let mut runtime = PluginRuntime::new("example");
    runtime.register_handler(Arc::new(HelloFastHandler));
    runtime.register_handler(Arc::new(HelloSlowHandler));
    runtime.run().await?;

    Ok(())
}
