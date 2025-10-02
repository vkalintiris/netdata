//! A runtime framework for building Netdata plugins with asynchronous function handlers.
//!
//! This crate provides a complete runtime system for creating Netdata plugins that can expose
//! custom functions to the Netdata monitoring system. It handles all the communication protocol,
//! serialization, concurrent execution, and lifecycle management.
//!
//! # Overview
//!
//! The framework is built around the [`FunctionHandler`] trait, which developers implement to
//! create custom functions that Netdata can call. The [`PluginRuntime`] manages these handlers
//! and provides:
//!
//! - Automatic JSON serialization/deserialization
//! - Concurrent function execution
//! - Graceful cancellation support
//! - Progress reporting capabilities
//! - Transaction management
//! - Clean shutdown handling
//!
//! # Example
//!
//! ```no_run
//! use async_trait::async_trait;
//! use netdata_plugin_error::Result;
//! use netdata_plugin_protocol::FunctionDeclaration;
//! use rt::{FunctionHandler, PluginRuntime};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Deserialize)]
//! struct MyRequest {
//!     name: String,
//! }
//!
//! #[derive(Serialize)]
//! struct MyResponse {
//!     greeting: String,
//! }
//!
//! struct MyHandler;
//!
//! #[async_trait]
//! impl FunctionHandler for MyHandler {
//!     type Request = MyRequest;
//!     type Response = MyResponse;
//!
//!     async fn on_call(&self, request: Self::Request) -> Result<Self::Response> {
//!         Ok(MyResponse {
//!             greeting: format!("Hello, {}!", request.name),
//!         })
//!     }
//!
//!     async fn on_cancellation(&self) -> Result<Self::Response> {
//!         Err(netdata_plugin_error::NetdataPluginError::Other {
//!             message: "Operation cancelled".to_string(),
//!         })
//!     }
//!
//!     async fn on_progress(&self) {
//!         // Report progress if needed
//!     }
//!
//!     fn declaration(&self) -> FunctionDeclaration {
//!         FunctionDeclaration::new("greet", "A greeting function")
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
//!     let mut runtime = PluginRuntime::new("my_plugin");
//!     runtime.register_handler(MyHandler);
//!     runtime.run().await?;
//!     Ok(())
//! }
//! ```
//!
//! # Architecture
//!
//! ## Communication Flow
//!
//! 1. Plugin declares available functions to Netdata via stdout
//! 2. Netdata sends function calls via stdin
//! 3. Runtime dispatches calls to registered handlers
//! 4. Handlers execute asynchronously with cancellation/progress support
//! 5. Results are sent back to Netdata via stdout
//!
//! ## Concurrency Model
//!
//! The runtime uses Tokio for asynchronous execution, allowing multiple function calls to be
//! processed concurrently. Each function call is tracked as a transaction with its own
//! cancellation token and control channel.

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

/// Internal control signals sent to running functions.
enum RuntimeSignal {
    /// Signal to request progress update from a running function.
    Progress,
}

/// Represents an active function call transaction.
///
/// Each transaction tracks a single function invocation, including its
/// unique identifier, control channel for signals, and cancellation token.
struct Transaction {
    /// Unique identifier for this transaction.
    id: String,
    /// Channel for sending control signals to the running function.
    control_tx: mpsc::Sender<RuntimeSignal>,
    /// Token for cancelling this specific function execution.
    cancellation_token: CancellationToken,
}

/// Execution context provided to function handlers.
///
/// Contains all the information and control mechanisms needed for
/// a function to execute, handle cancellation, and report progress.
struct FunctionContext {
    /// The original function call request from Netdata.
    function_call: Box<FunctionCall>,
    /// Token for detecting cancellation requests.
    cancellation_token: CancellationToken,
    /// Receiver for runtime control signals (e.g., progress requests).
    signal_rx: Mutex<mpsc::Receiver<RuntimeSignal>>,
}

/// Type alias for a future that produces a function result.
type FunctionFuture = BoxFuture<'static, (String, FunctionResult)>;

/// Trait for implementing Netdata function handlers.
///
/// This is the main trait that developers implement to create custom functions
/// that can be called by Netdata. The trait provides automatic serialization,
/// cancellation handling, and progress reporting.
///
/// # Type Parameters
///
/// * `Request` - The type of the incoming request payload (must be deserializable from JSON)
/// * `Response` - The type of the response payload (must be serializable to JSON)
///
/// # Example
///
/// ```
/// use async_trait::async_trait;
/// use netdata_plugin_error::Result;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Deserialize)]
/// struct AddRequest {
///     a: i32,
///     b: i32,
/// }
///
/// #[derive(Serialize)]
/// struct AddResponse {
///     sum: i32,
/// }
///
/// struct AddHandler;
///
/// #[async_trait]
/// impl FunctionHandler for AddHandler {
///     type Request = AddRequest;
///     type Response = AddResponse;
///
///     async fn on_call(&self, request: Self::Request) -> Result<Self::Response> {
///         Ok(AddResponse {
///             sum: request.a + request.b,
///         })
///     }
///
///     async fn on_cancellation(&self) -> Result<Self::Response> {
///         Err(netdata_plugin_error::NetdataPluginError::Other {
///             message: "Addition cancelled".to_string(),
///         })
///     }
///
///     async fn on_progress(&self) {
///         // Not needed for quick operations
///     }
///
///     fn declaration(&self) -> FunctionDeclaration {
///         FunctionDeclaration::new("add", "Adds two numbers")
///     }
/// }
/// ```
#[async_trait]
pub trait FunctionHandler: Send + Sync + 'static {
    /// The request payload type that will be deserialized from JSON.
    ///
    /// This type must implement `DeserializeOwned` to be deserializable from
    /// the JSON payload sent by Netdata.
    type Request: DeserializeOwned + Send;

    /// The response type that will be serialized to JSON.
    ///
    /// This type must implement `Serialize` to be serializable to JSON
    /// for sending back to Netdata.
    type Response: Serialize + Send;

    /// Main function logic executed when the function is called.
    ///
    /// This method contains the primary computation or operation that the
    /// function performs. It receives the deserialized request and should
    /// return either a successful response or an error.
    ///
    /// # Arguments
    ///
    /// * `request` - The deserialized request payload
    ///
    /// # Returns
    ///
    /// A `Result` containing either the response payload or an error
    ///
    /// # Cancellation
    ///
    /// This method may be interrupted if a cancellation is requested.
    /// When cancelled, the runtime will call [`on_cancellation`](Self::on_cancellation) instead.
    async fn on_call(&self, request: Self::Request) -> Result<Self::Response>;

    /// Handle cancellation requests while the function is running.
    ///
    /// Called when Netdata requests cancellation of a running function.
    /// This method should quickly return an appropriate error or partial result.
    ///
    /// # Returns
    ///
    /// Typically returns an error indicating the operation was cancelled,
    /// but may return a partial result if appropriate.
    async fn on_cancellation(&self) -> Result<Self::Response>;

    /// Handle progress report requests while the function is running.
    ///
    /// Called when Netdata requests a progress update from a long-running function.
    /// This method should log or report the current progress but doesn't need
    /// to return a value (progress is typically reported through logging).
    ///
    /// # Note
    ///
    /// This is called asynchronously while `on_call` is still running,
    /// so any shared state must be properly synchronized.
    async fn on_progress(&self);

    /// Provide the function's declaration metadata.
    ///
    /// Returns a [`FunctionDeclaration`] that describes this function to Netdata,
    /// including its name and description.
    ///
    /// # Returns
    ///
    /// A function declaration with the function's name and description.
    fn declaration(&self) -> FunctionDeclaration;
}

/// Internal trait for handling raw function calls with serialization.
///
/// This trait is used internally to bridge between the typed [`FunctionHandler`]
/// trait and the raw message protocol used by Netdata.
#[async_trait]
trait RawFunctionHandler: Send + Sync {
    /// Handle a raw function call with the given context.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The execution context containing the function call details
    ///
    /// # Returns
    ///
    /// A [`FunctionResult`] to be sent back to Netdata
    async fn handle_raw(&self, ctx: Arc<FunctionContext>) -> FunctionResult;

    /// Get the function declaration for this handler.
    fn declaration(&self) -> FunctionDeclaration;
}

/// Adapter that bridges typed handlers with the raw protocol.
///
/// This struct wraps a [`FunctionHandler`] implementation and provides
/// automatic JSON serialization/deserialization for the request and response.
struct HandlerAdapter<H: FunctionHandler> {
    handler: Arc<H>,
}

#[async_trait]
impl<H: FunctionHandler> RawFunctionHandler for HandlerAdapter<H> {
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
        let mut signal_rx = ctx.signal_rx.lock().await;

        let result = loop {
            tokio::select! {
                // Poll the main computation
                result = &mut call_future => {
                    break result;
                }
                // Handle progress requests
                Some(msg) = signal_rx.recv() => {
                    match msg {
                        RuntimeSignal::Progress => {
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

/// Main runtime for managing Netdata plugin execution.
///
/// The `PluginRuntime` orchestrates all aspects of a Netdata plugin's lifecycle:
/// - Registering function handlers
/// - Communicating with Netdata via stdin/stdout
/// - Managing concurrent function executions
/// - Handling cancellation and progress requests
/// - Graceful shutdown on signals
///
/// # Example
///
/// ```no_run
/// use rt::PluginRuntime;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let mut runtime = PluginRuntime::new("my_plugin");
///     // Register handlers here
///     runtime.run().await?;
///     Ok(())
/// }
/// ```
pub struct PluginRuntime {
    /// Name of this plugin (used for identification).
    plugin_name: String,
    /// Reader for incoming messages from Netdata (via stdin).
    reader: MessageReader<tokio::io::Stdin>,
    /// Writer for outgoing messages to Netdata (via stdout).
    writer: Arc<Mutex<MessageWriter<tokio::io::Stdout>>>,

    /// Registry of all available function handlers.
    function_handlers: HashMap<String, Arc<dyn RawFunctionHandler>>,

    /// Active transactions (ongoing function calls).
    transaction_registry: HashMap<String, Arc<Transaction>>,
    /// Futures representing running function executions.
    futures: FuturesUnordered<FunctionFuture>,

    /// Token for initiating graceful shutdown.
    shutdown_token: CancellationToken,
}

impl PluginRuntime {
    /// Create a new plugin runtime with the given name.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the plugin (used for identification in logs)
    ///
    /// # Returns
    ///
    /// A new `PluginRuntime` instance ready to accept handler registrations.
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

    /// Register a function handler with the runtime.
    ///
    /// The handler will be available for Netdata to call once the runtime starts.
    /// Multiple handlers can be registered, each with a unique function name.
    ///
    /// # Arguments
    ///
    /// * `handler` - The function handler implementation
    ///
    /// # Panics
    ///
    /// May panic if two handlers with the same function name are registered.
    pub fn register_handler<H: FunctionHandler + 'static>(&mut self, handler: H) {
        let adapter = HandlerAdapter {
            handler: Arc::new(handler),
        };
        let name = adapter.declaration().name.clone();
        self.function_handlers.insert(name, Arc::new(adapter));
    }

    /// Start the plugin runtime and begin processing messages.
    ///
    /// This method:
    /// 1. Sets up signal handlers for graceful shutdown
    /// 2. Declares all registered functions to Netdata
    /// 3. Enters the main message processing loop
    /// 4. Handles shutdown when requested
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on successful shutdown, or an error if a critical failure occurs.
    ///
    /// # Note
    ///
    /// This method runs indefinitely until shutdown is requested (via Ctrl-C or stdin closing).
    pub async fn run(mut self) -> Result<()> {
        info!("Starting plugin runtime: {}", self.plugin_name);

        self.handle_ctr_c();
        self.declare_functions().await?;
        self.process_messages().await?;
        self.shutdown().await?;

        Ok(())
    }

    /// Setup Ctrl-C signal handler for graceful shutdown.
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

    /// Declare all registered functions to Netdata.
    ///
    /// Sends a [`FunctionDeclaration`] message for each registered handler,
    /// informing Netdata about available functions and their metadata.
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

    /// Main message processing loop.
    ///
    /// Continuously processes incoming messages from Netdata and completed function futures.
    /// Handles function calls, cancellations, progress requests, and completions.
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

    /// Handle a single incoming message from Netdata.
    ///
    /// # Returns
    ///
    /// Returns `true` if the message loop should terminate, `false` otherwise.
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

    /// Handle an incoming function call request.
    ///
    /// Creates a new transaction, sets up the execution context, and spawns
    /// the function handler to process the request asynchronously.
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
            signal_rx: Mutex::new(control_rx),
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

    /// Handle a function cancellation request.
    ///
    /// Signals the corresponding transaction to cancel its execution.
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

    /// Handle a progress report request.
    ///
    /// Sends a progress signal to the corresponding running function.
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
        let _ = transaction.control_tx.send(RuntimeSignal::Progress).await;
    }

    /// Handle a completed function execution.
    ///
    /// Removes the transaction from the registry and sends the result back to Netdata.
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

    /// Perform graceful shutdown of the runtime.
    ///
    /// Cancels all active transactions and waits for them to complete
    /// (up to a timeout of 10 seconds). Any functions that don't complete
    /// within the timeout are forcefully aborted.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` after shutdown completes (either cleanly or after timeout).
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
