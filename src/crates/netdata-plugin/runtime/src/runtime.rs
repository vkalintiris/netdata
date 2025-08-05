use crate::{
    FunctionCall, FunctionContext, FunctionDeclaration, FunctionRegistry, FunctionResult,
    PluginContext, Result, Transaction,
};
use hyper_util::rt::TokioIo;
use netdata_plugin_proto::v1::agent::netdata_service_client::NetdataServiceClient;
use netdata_plugin_proto::v1::agent::netdata_service_server::{
    NetdataService, NetdataServiceServer,
};
use netdata_plugin_proto::v1::agent::DeclareFunctionResponse;
use netdata_plugin_proto::v1::plugin::plugin_service_server::{PluginService, PluginServiceServer};
use netdata_plugin_protocol::{Message, Transport};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{duplex, Stdin, Stdout};
use tokio::sync::Mutex;
use tonic::{
    transport::{Endpoint, Server, Uri},
    Request, Response, Status,
};
use tower::service_fn;
use tracing::{debug, error, info, warn};

type StdioTransport = Transport<Stdin, Stdout>;

/// Enhanced plugin runtime with function registry and context management
pub struct PluginRuntime {
    plugin_context: Arc<PluginContext>,
    registry: FunctionRegistry,
    transport: Arc<Mutex<StdioTransport>>,
    cleanup_interval: Duration,
}

impl PluginRuntime {
    /// Create a new enhanced plugin runtime
    pub fn new(plugin_name: impl Into<String>) -> Self {
        let plugin_name = plugin_name.into();

        Self {
            plugin_context: Arc::new(PluginContext::new(plugin_name)),
            registry: FunctionRegistry::new(),
            transport: Arc::new(Mutex::new(StdioTransport::new())),
            cleanup_interval: Duration::from_secs(30), // Cleanup expired transactions every 30s
        }
    }

    /// Register a function with its handler
    pub async fn register_function<F, Fut>(&self, function: FunctionDeclaration, handler: F)
    where
        F: Fn(Arc<PluginContext>, FunctionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = FunctionResult> + Send + 'static,
    {
        self.registry.register(function, handler).await;
    }

    /// Get the plugin context
    pub fn plugin_context(&self) -> &Arc<PluginContext> {
        &self.plugin_context
    }

    /// Get the function registry
    pub fn registry(&self) -> &FunctionRegistry {
        &self.registry
    }

    /// Run the enhanced plugin runtime
    pub async fn run(self) -> Result<()> {
        info!(
            "Starting {} plugin runtime (enhanced)...",
            self.plugin_context.plugin_name()
        );

        // Create in-memory duplex channels for both services
        let (agent_client_io, agent_server_io) = duplex(1024);
        let (_plugin_client_io, plugin_server_io) = duplex(1024);

        // Create service implementations with enhanced context and registry
        let netdata_service = NetdataServiceImpl {
            transport: self.transport.clone(),
        };

        let plugin_service = PluginServiceImpl {
            context: self.plugin_context.clone(),
            registry: self.registry.clone(),
        };

        // Spawn the agent gRPC server
        let agent_server_task = tokio::spawn(async move {
            info!("Starting agent gRPC server (enhanced)");
            Server::builder()
                .add_service(NetdataServiceServer::new(netdata_service))
                .serve_with_incoming(tokio_stream::once(Ok::<_, std::io::Error>(agent_server_io)))
                .await
                .map_err(|e| error!("Agent server error: {}", e))
                .unwrap_or(());
        });

        // Spawn the plugin gRPC server
        let plugin_server_task = tokio::spawn(async move {
            info!("Starting plugin gRPC server (enhanced)");
            Server::builder()
                .add_service(PluginServiceServer::new(plugin_service))
                .serve_with_incoming(tokio_stream::once(Ok::<_, std::io::Error>(
                    plugin_server_io,
                )))
                .await
                .map_err(|e| error!("Plugin server error: {}", e))
                .unwrap_or(());
        });

        // Create agent client
        let mut agent_client_io = Some(agent_client_io);
        let agent_channel = Endpoint::try_from("http://dummy")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let client = agent_client_io.take();
                async move {
                    if let Some(client) = client {
                        Ok(TokioIo::new(client))
                    } else {
                        Err(std::io::Error::other("Agent client already taken"))
                    }
                }
            }))
            .await?;

        let mut agent_client = NetdataServiceClient::new(agent_channel);

        // Declare all registered functions
        {
            let functions = self.registry.get_all_functions().await;
            for function in functions {
                let declaration = function.clone();
                let request = Request::new(declaration);

                match agent_client.declare_function(request).await {
                    Ok(response) => {
                        let resp = response.into_inner();
                        if resp.success {
                            info!("Successfully declared function: {}", function.name);
                        } else {
                            error!(
                                "Failed to declare function {}: {:?}",
                                function.name, resp.error_message
                            );
                        }
                    }
                    Err(e) => {
                        error!("Failed to declare function {}: {}", function.name, e);
                    }
                }
            }
        }

        // Start cleanup task for expired transactions
        let cleanup_context = self.plugin_context.clone();
        let cleanup_interval = self.cleanup_interval;
        let cleanup_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            loop {
                interval.tick().await;
                cleanup_context.cleanup_expired_transactions().await;
            }
        });

        // Store plugin name before moving self
        let plugin_name = self.plugin_context.plugin_name().to_string();

        // Handle incoming protocol messages from stdin
        let protocol_task = tokio::spawn(self.handle_protocol_input());

        // Wait for all tasks to complete
        tokio::select! {
            _ = agent_server_task => debug!("Agent server task completed"),
            _ = plugin_server_task => debug!("Plugin server task completed"),
            _ = cleanup_task => debug!("Cleanup task completed"),
            result = protocol_task => {
                if let Err(e) = result {
                    error!("Protocol handler task failed: {}", e);
                }
            }
        }

        info!("{} plugin runtime completed", plugin_name);
        Ok(())
    }

    /// Handle incoming protocol messages using enhanced transaction management
    async fn handle_protocol_input(self) -> Result<()> {
        let mut transport = StdioTransport::new();

        info!("Listening for protocol messages on stdin (enhanced)...");

        while let Some(message_result) = transport.recv().await {
            match message_result {
                Ok(message) => {
                    debug!("Received message: {:?}", message);

                    match message {
                        Message::FunctionCall(function_call) => {
                            info!(
                                "Received function call: {} (transaction: {})",
                                function_call.function, function_call.transaction
                            );

                            // Create and start transaction
                            let transaction = Transaction::new(
                                function_call.transaction.clone(),
                                function_call.function.clone(),
                                function_call.timeout,
                                function_call.source.clone(),
                                function_call.access,
                            );

                            self.plugin_context.start_transaction(transaction).await;

                            // Try to call the function using the registry
                            if let Some(mut result) = self
                                .registry
                                .call_function(self.plugin_context.clone(), *function_call.clone())
                                .await
                            {
                                result.transaction = function_call.transaction.clone();
                                self.plugin_context
                                    .complete_transaction(&function_call.transaction)
                                    .await;

                                // Send response using the transport
                                if let Err(e) = transport
                                    .send(Message::FunctionResult(Box::new(result)))
                                    .await
                                {
                                    error!("Failed to send response: {}", e);
                                    self.plugin_context
                                        .fail_transaction(&function_call.transaction)
                                        .await;
                                }
                            } else {
                                warn!("Function not found: {}", function_call.function);
                                self.plugin_context
                                    .fail_transaction(&function_call.transaction)
                                    .await;

                                // Send error response
                                let error_result = FunctionResult {
                                    transaction: function_call.transaction,
                                    status: 404,
                                    format: "text/plain".to_string(),
                                    expires: 0,
                                    payload: format!(
                                        "Function '{}' not found",
                                        function_call.function
                                    )
                                    .into_bytes(),
                                };

                                if let Err(e) = transport
                                    .send(Message::FunctionResult(Box::new(error_result)))
                                    .await
                                {
                                    error!("Failed to send error response: {}", e);
                                }
                            }
                        }
                        Message::FunctionCancel(cancel) => {
                            info!(
                                "Received function cancel for transaction: {}",
                                cancel.transaction
                            );
                            self.plugin_context
                                .cancel_transaction(&cancel.transaction)
                                .await;
                            // TODO: Implement function cancellation in registry
                        }
                        other => {
                            debug!("Received other message type: {:?}", other);
                        }
                    }
                }
                Err(e) => {
                    error!("Error parsing protocol message: {:?}", e);
                    // Continue processing other messages
                }
            }
        }

        info!("Protocol input handling ended");
        Ok(())
    }
}

/// Enhanced NetdataService implementation
struct NetdataServiceImpl {
    transport: Arc<Mutex<StdioTransport>>,
}

#[tonic::async_trait]
impl NetdataService for NetdataServiceImpl {
    async fn declare_function(
        &self,
        request: Request<netdata_plugin_proto::v1::FunctionDeclaration>,
    ) -> std::result::Result<Response<DeclareFunctionResponse>, Status> {
        let declaration = request.into_inner();

        info!("Received function declaration: {}", declaration.name);

        let message = Message::FunctionDeclaration(Box::new(declaration));

        if let Err(e) = self.send_message(message).await {
            error!("Failed to send function declaration: {}", e);
            return Err(Status::internal("Failed to send function declaration"));
        }

        let response = DeclareFunctionResponse {
            success: true,
            error_message: None,
        };

        Ok(Response::new(response))
    }
}

impl NetdataServiceImpl {
    async fn send_message(
        &self,
        message: Message,
    ) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut transport = self.transport.lock().await;
        transport.send(message).await?;
        Ok(())
    }
}

/// Enhanced PluginService implementation with context and registry
struct PluginServiceImpl {
    context: Arc<PluginContext>,
    registry: FunctionRegistry,
}

#[tonic::async_trait]
impl PluginService for PluginServiceImpl {
    async fn call_function(
        &self,
        request: Request<FunctionCall>,
    ) -> std::result::Result<Response<FunctionResult>, Status> {
        let function_call = request.into_inner();

        info!(
            "Plugin service received function call: {} (transaction: {})",
            function_call.function, function_call.transaction
        );

        // Create and start transaction
        let transaction = Transaction::new(
            function_call.transaction.clone(),
            function_call.function.clone(),
            function_call.timeout,
            function_call.source.clone(),
            function_call.access,
        );

        self.context.start_transaction(transaction).await;

        // Try to call the function using the registry
        let call = function_call.clone();

        if let Some(mut result) = self
            .registry
            .call_function(self.context.clone(), call)
            .await
        {
            result.transaction = function_call.transaction.clone();
            self.context
                .complete_transaction(&function_call.transaction)
                .await;

            Ok(Response::new(result))
        } else {
            warn!("Function not found: {}", function_call.function);
            self.context
                .fail_transaction(&function_call.transaction)
                .await;

            let error_result = FunctionResult {
                transaction: function_call.transaction,
                status: 404,
                format: "text/plain".to_string(),
                expires: 0,
                payload: format!("Function '{}' not found", function_call.function).into_bytes(),
            };

            Ok(Response::new(error_result))
        }
    }
}
