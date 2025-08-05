use crate::{FunctionCall, TransactionId};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// Context provided to function handlers containing call details
#[derive(Debug, Clone)]
pub struct FunctionContext {
    call: FunctionCall,
    started_at: Instant,
    cancellation_token: CancellationToken,
}

impl FunctionContext {
    /// Create a new function context from a function call
    pub fn new(call: FunctionCall) -> Self {
        Self {
            call,
            started_at: Instant::now(),
            cancellation_token: CancellationToken::new(),
        }
    }

    /// Create a new function context with a specific cancellation token
    pub fn with_cancellation(call: FunctionCall, cancellation_token: CancellationToken) -> Self {
        Self {
            call,
            started_at: Instant::now(),
            cancellation_token,
        }
    }

    /// Get the transaction ID
    pub fn transaction_id(&self) -> &TransactionId {
        &self.call.transaction
    }

    /// Get the function name
    pub fn function_name(&self) -> &str {
        &self.call.name
    }

    /// Get the timeout in seconds
    pub fn timeout(&self) -> u32 {
        self.call.timeout
    }

    /// Get the access level
    pub fn access(&self) -> Option<u32> {
        self.call.access
    }

    /// Get the source of the function call
    pub fn source(&self) -> Option<&str> {
        self.call.source.as_deref()
    }

    /// Get the payload if present
    pub fn payload(&self) -> Option<&[u8]> {
        self.call.payload.as_deref()
    }

    /// Get the elapsed time since the function was called
    pub fn elapsed(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }

    /// Get the underlying function call
    pub fn call(&self) -> &FunctionCall {
        &self.call
    }

    /// Consume the context and return the underlying function call
    pub fn into_call(self) -> FunctionCall {
        self.call
    }

    /// Get the cancellation token
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation_token
    }

    /// Check if the function has been cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }
}
