#![allow(unused_imports)]

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::{
    FunctionCall, FunctionDeclaration, FunctionResult, Message, MessageReader, MessageWriter,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

pub enum ControlMessage {
    Progress,
    Cancel,
}

pub struct FunctionContext {
    pub function_call: Box<FunctionCall>,
    pub control_tx: mpsc::Sender<ControlMessage>,
    pub control_rx: mpsc::Receiver<ControlMessage>,
}

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
    function_progress: HashMap<String, Arc<dyn FunctionHandler>>,

    call_registry: HashMap<String, Arc<FunctionContext>>,
    futures:
        FuturesUnordered<Pin<Box<dyn futures::Future<Output = (String, FunctionResult)> + Send>>>,

    shutdown_token: CancellationToken,
}

impl PluginRuntime {
    pub fn new(name: &str) -> Self {
        Self {
            plugin_name: String::from(name),
            reader: MessageReader::new(tokio::io::stdin()),
            writer: Arc::new(Mutex::new(MessageWriter::new(tokio::io::stdout()))),

            function_handlers: HashMap::new(),
            function_progress: HashMap::new(),

            call_registry: HashMap::new(),
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

    fn handle_function_call(&mut self, function_call: Box<FunctionCall>) {
        if self.call_registry.contains_key(&function_call.transaction) {
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
            control_tx,
            control_rx,
        });
        let transaction = function_context.function_call.transaction.clone();
        self.call_registry
            .insert(transaction.clone(), function_context.clone());

        let future = Box::pin(async move {
            let result = handler.handle(function_context).await;
            (transaction, result)
        });
        self.futures.push(future);
    }

    async fn handle_message(&mut self, message: Option<Result<Message>>) -> Result<bool> {
        match message {
            Some(Ok(Message::FunctionCall(function_call))) => {
                self.handle_function_call(function_call);
            }
            Some(Ok(Message::FunctionCancel(function_cancel))) => {
                if let Some(function_context) = self.call_registry.get(&function_cancel.transaction)
                {
                    let _ = function_context
                        .control_tx
                        .send(ControlMessage::Cancel)
                        .await;
                }
            }
            Some(Ok(Message::FunctionProgress(function_progress))) => {
                if let Some(function_context) =
                    self.call_registry.get(&function_progress.transaction)
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

    async fn handle_completed(
        &mut self,
        transaction: String,
        result: FunctionResult,
    ) -> Result<()> {
        self.call_registry.remove(&transaction);
        self.writer
            .lock()
            .await
            .send(Message::FunctionResult(Box::new(result)))
            .await?;
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
}
