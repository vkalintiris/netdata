#![allow(unused_imports)]

use async_trait::async_trait;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::{
    FunctionCall, FunctionCancel, FunctionDeclaration, FunctionProgress, FunctionResult, Message,
    MessageReader, MessageWriter,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

pub enum ControlMessage {
    Progress,
}

struct Transaction {
    id: String,
    control_tx: mpsc::Sender<ControlMessage>,
    cancellation_token: CancellationToken,
}

pub struct FunctionContext {
    pub function_call: Box<FunctionCall>,
    pub cancellation_token: CancellationToken,
    pub control_rx: Mutex<mpsc::Receiver<ControlMessage>>,
}

pub struct FunctionRequest<T> {
    pub context: Arc<FunctionContext>,
    pub payload: T,
}

type FunctionFuture = BoxFuture<'static, (String, FunctionResult)>;

/// Trait for implementing function handlers with automatic cancellation and progress management
#[async_trait]
pub trait FunctionHandler: Send + Sync + 'static {
    /// The request payload type that will be deserialized from JSON
    type Request: DeserializeOwned + Send;

    /// The response type that will be serialized to JSON
    type Response: Serialize + Send;

    /// Called when the function is invoked - contains the main computation
    /// Returns a future that produces the response
    async fn on_call(&self, request: Self::Request) -> Result<Self::Response>;

    /// Called when cancellation is requested while on_call is running
    async fn on_cancellation(&self) -> Result<Self::Response>;

    /// Called when progress report is requested while on_call is running
    async fn on_progress(&self);

    /// Get the function declaration for this handler
    fn declaration(&self) -> FunctionDeclaration;
}

/// Internal trait that handles raw function calls with serialization/deserialization
#[async_trait]
trait FunctionHandlerInternal: Send + Sync {
    async fn handle_raw(&self, ctx: Arc<FunctionContext>) -> FunctionResult;
    fn declaration(&self) -> FunctionDeclaration;
}

/// Adapter that wraps a FunctionHandler and provides serialization/deserialization
struct HandlerAdapter<H: FunctionHandler> {
    handler: Arc<H>,
}

#[async_trait]
impl<H: FunctionHandler> FunctionHandlerInternal for HandlerAdapter<H> {
    async fn handle_raw(&self, ctx: Arc<FunctionContext>) -> FunctionResult {
        let transaction = ctx.function_call.transaction.clone();

        // Deserialize the request payload
        let payload: H::Request = match &ctx.function_call.payload {
            Some(bytes) => match serde_json::from_slice(bytes) {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to deserialize request payload: {}", e);
                    return FunctionResult {
                        transaction,
                        status: 400,
                        expires: 0,
                        format: "text/plain".to_string(),
                        payload: format!("Invalid request: {}", e).as_bytes().to_vec(),
                    };
                }
            },
            None => match serde_json::from_slice(b"{}") {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to deserialize empty payload: {}", e);
                    return FunctionResult {
                        transaction,
                        status: 400,
                        expires: 0,
                        format: "text/plain".to_string(),
                        payload: format!("Invalid request: {}", e).as_bytes().to_vec(),
                    };
                }
            },
        };

        // Drive the handler with cancellation and progress handling
        let handler = self.handler.clone();

        let mut call_future = Box::pin(handler.on_call(payload));
        let mut control_rx = ctx.control_rx.lock().await;

        let result = loop {
            tokio::select! {
                // Poll the main computation
                result = &mut call_future => {
                    break result;
                }
                // Handle progress requests
                Some(msg) = control_rx.recv() => {
                    match msg {
                        ControlMessage::Progress => {
                            handler.on_progress().await;
                        }
                    }
                }
                // Handle cancellation
                _ = ctx.cancellation_token.cancelled() => {
                    break handler.on_cancellation().await;
                }
            }
        };

        // Process the result
        match result {
            Ok(response) => {
                // Serialize the response
                match serde_json::to_vec(&response) {
                    Ok(payload) => FunctionResult {
                        transaction,
                        status: 200,
                        expires: 0,
                        format: "application/json".to_string(),
                        payload,
                    },
                    Err(e) => {
                        error!("Failed to serialize response: {}", e);
                        FunctionResult {
                            transaction,
                            status: 500,
                            expires: 0,
                            format: "text/plain".to_string(),
                            payload: format!("Serialization error: {}", e).as_bytes().to_vec(),
                        }
                    }
                }
            }
            Err(e) => {
                error!("Handler error: {}", e);
                FunctionResult {
                    transaction,
                    status: 500,
                    expires: 0,
                    format: "text/plain".to_string(),
                    payload: format!("Handler error: {}", e).as_bytes().to_vec(),
                }
            }
        }
    }

    fn declaration(&self) -> FunctionDeclaration {
        self.handler.declaration()
    }
}

pub struct PluginRuntime {
    plugin_name: String,
    reader: MessageReader<tokio::io::Stdin>,
    writer: Arc<Mutex<MessageWriter<tokio::io::Stdout>>>,

    function_handlers: HashMap<String, Arc<dyn FunctionHandlerInternal>>,

    transaction_registry: HashMap<String, Arc<Transaction>>,
    futures: FuturesUnordered<FunctionFuture>,

    shutdown_token: CancellationToken,
}

impl PluginRuntime {
    pub fn new(name: &str) -> Self {
        Self {
            plugin_name: String::from(name),
            reader: MessageReader::new(tokio::io::stdin()),
            writer: Arc::new(Mutex::new(MessageWriter::new(tokio::io::stdout()))),

            function_handlers: HashMap::new(),
            transaction_registry: HashMap::new(),
            futures: FuturesUnordered::new(),

            shutdown_token: CancellationToken::default(),
        }
    }

    pub fn register_handler<H: FunctionHandler + 'static>(&mut self, handler: H) {
        let adapter = HandlerAdapter {
            handler: Arc::new(handler),
        };
        let name = adapter.declaration().name.clone();
        self.function_handlers.insert(name, Arc::new(adapter));
    }

    pub async fn run(mut self) -> Result<()> {
        info!("Starting plugin runtime: {}", self.plugin_name);

        self.handle_ctr_c();
        self.declare_functions().await?;
        self.process_messages().await?;
        self.shutdown().await?;

        Ok(())
    }

    // Setup Ctrl-C signal handler
    fn handle_ctr_c(&self) {
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
    }

    /// Declare all registered functions to Netdata
    async fn declare_functions(&self) -> Result<()> {
        let mut writer = self.writer.lock().await;

        for (name, handler) in self.function_handlers.iter() {
            info!("Declaring function: {}", name);

            let message = Message::FunctionDeclaration(Box::new(handler.declaration()));
            if let Err(e) = writer.send(message).await {
                error!("Failed to declare function {}: {}", name, e);
                return Err(e);
            }
        }

        writer.flush().await?;

        Ok(())
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
                    if self.handle_message(message).await? {
                        break;
                    }
                }
                Some((transaction, result)) = self.futures.next() => {
                    self.handle_completed(transaction, result).await?;
                }
            }
        }

        Ok(())
    }

    async fn handle_message(&mut self, message: Option<Result<Message>>) -> Result<bool> {
        match message {
            Some(Ok(Message::FunctionCall(function_call))) => {
                self.handle_function_call(function_call);
            }
            Some(Ok(Message::FunctionCancel(function_cancel))) => {
                self.handle_function_cancel(function_cancel.as_ref());
            }
            Some(Ok(Message::FunctionProgress(function_progress))) => {
                self.handle_function_progress(&function_progress).await;
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
                return Ok(true); // Signal to break the loop
            }
        }
        Ok(false)
    }

    fn handle_function_call(&mut self, function_call: Box<FunctionCall>) {
        if self
            .transaction_registry
            .contains_key(&function_call.transaction)
        {
            warn!(
                "Ignoring existing transaction {:#?} for function {:#?}",
                function_call.transaction, function_call.name
            );
            return;
        }

        // Get handler
        let Some(handler) = self.function_handlers.get(&function_call.name).cloned() else {
            error!("Could not find function {:#?}", function_call.name);
            return;
        };

        // Create a new function context
        let (control_tx, control_rx) = mpsc::channel(4);
        let cancellation_token = CancellationToken::new();

        let function_context = Arc::new(FunctionContext {
            function_call,
            cancellation_token: cancellation_token.clone(),
            control_rx: Mutex::new(control_rx),
        });

        // Create new transaction
        let id = function_context.function_call.transaction.clone();
        let transaction = Arc::new(Transaction {
            id,
            cancellation_token,
            control_tx,
        });
        self.transaction_registry
            .insert(transaction.id.clone(), transaction.clone());

        // Create future
        let future = Box::pin(async move {
            let result = handler.handle_raw(function_context).await;
            (transaction.id.clone(), result)
        });
        self.futures.push(future);
    }

    fn handle_function_cancel(&mut self, function_cancel: &FunctionCancel) {
        let Some(transaction) = self.transaction_registry.get(&function_cancel.transaction) else {
            warn!(
                "Can not cancel non-existing transaction {}",
                function_cancel.transaction
            );
            return;
        };

        info!("Cancelling transaction {}", function_cancel.transaction);
        transaction.cancellation_token.cancel();
    }

    async fn handle_function_progress(&mut self, function_progress: &FunctionProgress) {
        let Some(transaction) = self
            .transaction_registry
            .get(&function_progress.transaction)
        else {
            warn!(
                "Can not get progress of non-existing transaction {}",
                function_progress.transaction
            );
            return;
        };

        info!(
            "Requesting progress of transaction {}",
            function_progress.transaction
        );
        let _ = transaction.control_tx.send(ControlMessage::Progress).await;
    }

    async fn handle_completed(
        &mut self,
        transaction: String,
        result: FunctionResult,
    ) -> Result<()> {
        self.transaction_registry.remove(&transaction);
        self.writer
            .lock()
            .await
            .send(Message::FunctionResult(Box::new(result)))
            .await?;
        Ok(())
    }

    /// Shutdown the runtime gracefully with a timeout
    async fn shutdown(&mut self) -> Result<()> {
        let in_flight = self.transaction_registry.len();

        if in_flight == 0 {
            info!("Clean shutdown - no in-flight functions");
            return Ok(());
        }

        info!("Shutting down with {} in-flight functions...", in_flight);

        // Send cancel to all active transactions
        for transaction in self.transaction_registry.values() {
            transaction.cancellation_token.cancel();
        }

        // Wait for functions to complete with a timeout
        let timeout = Duration::from_secs(10);
        let mut completed = 0;

        match tokio::time::timeout(timeout, async {
            while let Some((transaction, result)) = self.futures.next().await {
                if let Err(e) = self.handle_completed(transaction, result).await {
                    error!("Error handling completed function during shutdown: {}", e);
                }
                completed += 1;
            }
        })
        .await
        {
            Ok(()) => {
                info!("Clean shutdown - all {} functions completed", completed);
            }
            Err(_) => {
                let aborted = in_flight - completed;
                warn!(
                    "Shutdown timeout - {} functions completed, {} aborted",
                    completed, aborted
                );
            }
        }

        Ok(())
    }
}
