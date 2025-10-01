#![allow(unused_imports)]

use async_trait::async_trait;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::{
    FunctionCall, FunctionDeclaration, FunctionResult, Message, MessageReader, MessageWriter,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

pub enum ControlMessage {
    Progress,
    Cancel,
}

struct Transaction {
    id: String,
    control_tx: mpsc::Sender<ControlMessage>,
}

pub struct FunctionContext {
    pub function_call: Box<FunctionCall>,
    pub control_rx: Mutex<mpsc::Receiver<ControlMessage>>,
}

type FunctionFuture = BoxFuture<'static, (String, FunctionResult)>;

#[async_trait]
pub trait FunctionHandler: Send + Sync {
    /// Handle a function call
    async fn handle(&self, ctx: Arc<FunctionContext>) -> FunctionResult;

    /// Report progress
    async fn progress(&self, ctx: FunctionContext) -> FunctionResult;

    /// Get the function declaration for this handler
    fn declaration(&self) -> FunctionDeclaration;
}

pub struct PluginRuntime {
    plugin_name: String,
    reader: MessageReader<tokio::io::Stdin>,
    writer: Arc<Mutex<MessageWriter<tokio::io::Stdout>>>,

    function_handlers: HashMap<String, Arc<dyn FunctionHandler>>,

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

    pub fn register_handler(&mut self, handler: Arc<dyn FunctionHandler>) {
        let name = handler.declaration().name;
        self.function_handlers.insert(name, handler);
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
                if let Some(function_context) =
                    self.transaction_registry.get(&function_cancel.transaction)
                {
                    let _ = function_context
                        .control_tx
                        .send(ControlMessage::Cancel)
                        .await;
                }
            }
            Some(Ok(Message::FunctionProgress(function_progress))) => {
                if let Some(function_context) = self
                    .transaction_registry
                    .get(&function_progress.transaction)
                {
                    let _ = function_context
                        .control_tx
                        .send(ControlMessage::Progress)
                        .await;
                }
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

        let Some(handler) = self.function_handlers.get(&function_call.name) else {
            error!("Could not find function {:#?}", function_call.name);
            return;
        };
        let handler = handler.clone();

        let (control_tx, control_rx) = mpsc::channel(4);

        let function_context = Arc::new(FunctionContext {
            function_call,
            control_rx: Mutex::new(control_rx),
        });

        let id = function_context.function_call.transaction.clone();
        let transaction = Arc::new(Transaction { id, control_tx });
        self.transaction_registry
            .insert(transaction.id.clone(), transaction.clone());

        let future = Box::pin(async move {
            let result = handler.handle(function_context).await;
            (transaction.id.clone(), result)
        });
        self.futures.push(future);
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
            let _ = transaction.control_tx.send(ControlMessage::Cancel).await;
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
