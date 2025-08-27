#![allow(dead_code)]

use crate::config_registry::{Config, ConfigRegistry};
use crate::{
    ConfigDeclarable, FunctionCall, FunctionCancel, FunctionContext, FunctionDeclaration,
    FunctionRegistry, FunctionResult, PluginContext, Result, RuntimeError,
};
use futures::StreamExt;
use netdata_plugin_protocol::{DynCfgCmds, Message, MessageReader, MessageWriter};
use netdata_plugin_schema::NetdataSchema;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// The main plugin runtime that handles Netdata protocol messages
pub struct PluginRuntime {
    plugin_name: String,
    config_registry: ConfigRegistry,
    function_registry: FunctionRegistry,
    plugin_context: PluginContext,
    reader: MessageReader<tokio::io::Stdin>,
    writer: Arc<Mutex<MessageWriter<tokio::io::Stdout>>>,
    active_handlers: Arc<Mutex<JoinSet<()>>>,
    runtime_tasks: Arc<Mutex<JoinSet<()>>>,
    shutdown_token: CancellationToken,
}

impl PluginRuntime {
    /// Create a new plugin runtime
    pub fn new(plugin_name: impl Into<String>) -> Self {
        let plugin_name = plugin_name.into();
        let plugin_context = PluginContext::new(plugin_name.clone());
        let config_registry = ConfigRegistry::default();
        let function_registry = FunctionRegistry::new();
        let reader = MessageReader::new(tokio::io::stdin());
        let writer = Arc::new(Mutex::new(MessageWriter::new(tokio::io::stdout())));
        let active_handlers = Arc::new(Mutex::new(JoinSet::new()));
        let runtime_tasks = Arc::new(Mutex::new(JoinSet::new()));
        let shutdown_token = CancellationToken::new();

        Self {
            plugin_name,
            config_registry,
            function_registry,
            plugin_context,
            reader,
            writer,
            active_handlers,
            runtime_tasks,
            shutdown_token,
        }
    }

    pub async fn register_config_functions(&self, cfg: Config) {
        let cmds = cfg.dyncfg_commands();

        if cmds.contains(DynCfgCmds::SCHEMA) {
            // setup function handler for retrieving configuration schema
            let id = String::from(cfg.id());
            let name = format!("config {} schema", id);
            let help = format!("Retrieve configuration schema for '{}'", id);

            let declaration = FunctionDeclaration {
                name,
                help,
                global: false,
                timeout: 10,
                tags: None,
                access: None,
                priority: None,
                version: None,
            };

            let handler = async |plugin_ctx: PluginContext, fn_ctx: FunctionContext| {
                let id = fn_ctx.function_name().split_whitespace().nth(1).unwrap();
                let cfg = plugin_ctx.get_config(id).await.unwrap();
                let payload = serde_json::to_vec_pretty(cfg.schema()).unwrap();

                FunctionResult {
                    transaction: fn_ctx.transaction_id().clone(),
                    status: 200,
                    format: "application/json".to_string(),
                    expires: 0,
                    payload,
                }
            };

            self.register_function(declaration, handler).await.unwrap();
        }

        if cmds.contains(DynCfgCmds::GET) {
            // setup function handler for retrieving config value
            let id = String::from(cfg.id());
            let name = format!("config {} get", id);
            let help = format!("Get configuration value for '{}'", id);

            let declaration = FunctionDeclaration {
                name,
                help,
                global: false,
                timeout: 10,
                tags: None,
                access: None,
                priority: None,
                version: None,
            };

            let handler = async |plugin_ctx: PluginContext, fn_ctx: FunctionContext| {
                let id = fn_ctx.function_name().split_whitespace().nth(1).unwrap();
                let cfg = plugin_ctx.get_config(id).await.unwrap();
                let initial_value = cfg.initial_value().expect("WTF?");
                let payload = serde_json::to_vec_pretty(initial_value).unwrap();

                FunctionResult {
                    transaction: fn_ctx.transaction_id().clone(),
                    status: 200,
                    format: "application/json".to_string(),
                    expires: 0,
                    payload,
                }
            };

            self.register_function(declaration, handler).await.unwrap();
        }
    }

    pub async fn register_config<T: ConfigDeclarable + NetdataSchema>(
        &self,
        initial_value: Option<T>,
    ) -> Result<()> {
        let cfg = Config::new::<T>(initial_value);
        self.plugin_context.insert_config(cfg.clone()).await;
        self.register_config_functions(cfg).await;

        Ok(())
    }

    /// Register a function with its handler
    pub async fn register_function<F, Fut>(
        &self,
        declaration: FunctionDeclaration,
        handler: F,
    ) -> Result<()>
    where
        F: Fn(PluginContext, FunctionContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = FunctionResult> + Send + 'static,
    {
        self.function_registry.register(declaration, handler).await;
        Ok(())
    }

    /// Declare all registered functions to Netdata
    async fn declare_functions(&self) -> Result<()> {
        let functions = self.function_registry.get_all_declarations().await;
        let mut writer = self.writer.lock().await;

        for declaration in functions {
            info!("Declaring function: {}", declaration.name);

            let message = Message::FunctionDeclaration(Box::new(declaration.clone()));
            if let Err(e) = writer.send(message).await {
                error!("Failed to declare function {}: {}", declaration.name, e);
                return Err(RuntimeError::Transport(Box::new(e)));
            }
        }

        writer
            .flush()
            .await
            .map_err(|e| RuntimeError::Transport(Box::new(e)))?;

        Ok(())
    }

    /// Handle a function call
    async fn handle_function_call(&self, call: Box<FunctionCall>) {
        let transaction_id = call.transaction.clone();
        let function_name = call.name.clone();

        info!(
            "Handling function call: {} (transaction: {})",
            function_name, transaction_id
        );

        // Start transaction - check for duplicates
        let transaction_started = self
            .plugin_context
            .start_transaction(
                transaction_id.clone(),
                function_name.clone(),
                call.timeout,
                call.source.clone(),
                call.access,
            )
            .await;

        // If transaction already exists, return an error
        if !transaction_started {
            warn!("Duplicate transaction ID: {}", transaction_id);
            let error_result = FunctionResult {
                transaction: transaction_id,
                status: 409, // Conflict
                format: "text/plain".to_string(),
                expires: 0,
                payload: b"Duplicate transaction ID".to_vec(),
            };

            self.send_result(error_result).await;
            return;
        }

        // Create cancellation token for this handler
        let handler_token = self.shutdown_token.child_token();

        // Clone what we need for the async task
        let registry = self.function_registry.clone();
        let context = self.plugin_context.clone();
        let writer = self.writer.clone();
        let transaction_id_clone = transaction_id.clone();

        // Spawn the handler task
        let mut handlers = self.active_handlers.lock().await;
        handlers.spawn(async move {
            // Execute function via registry
            let result = match registry
                .call_function(context.clone(), *call, handler_token)
                .await
            {
                Some(mut result) => {
                    result.transaction = transaction_id_clone.clone();
                    context.complete_transaction(&transaction_id_clone).await;
                    result
                }
                None => {
                    context.fail_transaction(&transaction_id_clone).await;
                    FunctionResult {
                        transaction: transaction_id_clone,
                        status: 404,
                        format: "text/plain".to_string(),
                        expires: 0,
                        payload: format!("Function '{}' not found", function_name).into_bytes(),
                    }
                }
            };

            // Send result
            debug!("Sending result for transaction: {}", result.transaction);
            let mut w = writer.lock().await;
            if let Err(e) = w.send(Message::FunctionResult(Box::new(result))).await {
                error!("Failed to send function result: {}", e);
            } else if let Err(e) = w.flush().await {
                error!("Failed to flush writer: {}", e);
            }
        });
    }

    /// Handle a function cancel request
    async fn handle_function_cancel(&self, cancel: Box<FunctionCancel>) {
        let transaction_id = &cancel.transaction;
        info!("Cancelling function: {}", transaction_id);

        // Cancel the transaction in the context
        self.plugin_context.cancel_transaction(transaction_id).await;

        // Note: The handler should detect cancellation via:
        // 1. plugin_context.is_transaction_cancelled()
        // 2. function_context.is_cancelled()
    }

    /// Send a result to stdout
    async fn send_result(&self, result: FunctionResult) {
        let mut writer = self.writer.lock().await;
        if let Err(e) = writer.send(Message::FunctionResult(Box::new(result))).await {
            error!("Failed to send result: {}", e);
        } else if let Err(e) = writer.flush().await {
            error!("Failed to flush writer: {}", e);
        }
    }

    /// Main message processing loop
    async fn process_messages(&mut self) -> Result<()> {
        info!("Starting message processing loop");

        loop {
            tokio::select! {
                // Make the shutdown signal higher priority by putting it first
                _ = self.shutdown_token.cancelled() => {
                    info!("Shutdown requested... Stop processing messages from stdin");
                    // Reader will be dropped here, closing stdin
                    break;
                }
                message = self.reader.next() => {
                    match message {
                        Some(Ok(Message::FunctionCall(call))) => {
                            self.handle_function_call(call).await;
                        }
                        Some(Ok(Message::FunctionCancel(cancel))) => {
                            self.handle_function_cancel(cancel).await;
                        }
                        Some(Ok(msg)) => {
                            debug!("Received message: {:?}", msg);
                        }
                        Some(Err(e)) => {
                            error!("Error parsing message: {:?}", e);
                        }
                        None => {
                            info!("Input stream ended");
                            self.shutdown_token.cancel();
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Wait for active handlers to complete
    async fn wait_for_handlers(&self, timeout: Duration) {
        info!("Waiting for any active function handlers to complete...");

        let start = std::time::Instant::now();

        loop {
            let handlers_count = {
                let mut handlers = self.active_handlers.lock().await;
                // Clean up finished tasks
                while let Some(result) = handlers.try_join_next() {
                    match result {
                        Ok(()) => {}
                        Err(e) => warn!("Handler task failed: {}", e),
                    }
                }
                handlers.len()
            };

            if handlers_count == 0 {
                info!("All function handlers completed");
                break;
            }

            if start.elapsed() >= timeout {
                warn!(
                    "Shutdown timeout reached, {} handlers still active",
                    handlers_count
                );
                break;
            }

            debug!(
                "{} function handlers still active, waiting...",
                handlers_count
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Run the plugin runtime
    pub async fn run(mut self) -> Result<()> {
        info!("Starting plugin runtime: {}", self.plugin_name);

        // Set up Ctrl-C handler - spawn directly, not in runtime_tasks
        let shutdown_token = self.shutdown_token.clone();
        tokio::spawn(async move {
            match tokio::signal::ctrl_c().await {
                Ok(()) => {
                    info!("Received Ctrl-C signal, initiating graceful shutdown");
                    shutdown_token.cancel();
                }
                Err(e) => {
                    error!("Failed to listen for Ctrl-C signal: {}", e);
                }
            }
        });

        // Start transaction cleanup task
        {
            let mut tasks = self.runtime_tasks.lock().await;
            let cleanup_context = self.plugin_context.clone();
            let cleanup_token = self.shutdown_token.clone();
            tasks.spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            cleanup_context.cleanup_expired_transactions().await;
                        }
                        _ = cleanup_token.cancelled() => {
                            debug!("Transaction cleanup task shutting down");
                            break;
                        }
                    }
                }
            });
        }

        // Declare functions
        self.declare_functions().await?;

        // Process messages - reader will be dropped when this returns
        self.process_messages().await?;

        // Graceful shutdown: wait for active handlers
        self.wait_for_handlers(Duration::from_secs(30)).await;

        // Abort all runtime tasks
        {
            let mut tasks = self.runtime_tasks.lock().await;
            debug!("Aborting {} runtime tasks", tasks.len());
            tasks.abort_all();
            while tasks.join_next().await.is_some() {
                // Drain all tasks
            }
        }

        info!("Plugin runtime stopped");
        Ok(())
    }
}
