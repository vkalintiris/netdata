use crate::{FunctionCall, TransactionId};
use std::collections::HashMap;
use tracing::debug;

//
/// Function-specific context prepared by the runtime for each function call
#[derive(Debug, Clone)]
pub struct FunctionContext {
    /// The original function call
    call: FunctionCall,
    /// Parsed parameters from the function call
    parameters: HashMap<String, String>,
    /// Function-specific metadata
    metadata: FunctionMetadata,
}

/// Metadata about the current function execution
#[derive(Debug, Clone)]
pub struct FunctionMetadata {
    /// When this function call started
    pub start_time: std::time::SystemTime,
    /// Function call attempt number (for retries)
    pub attempt: u32,
}

impl FunctionContext {
    /// Create a new function context from a function call
    pub fn new(call: FunctionCall) -> Self {
        let parameters = Self::parse_parameters(&call);
        let metadata = FunctionMetadata {
            start_time: std::time::SystemTime::now(),
            attempt: 1,
        };

        Self {
            call,
            parameters,
            metadata,
        }
    }

    /// Get the transaction ID
    pub fn transaction_id(&self) -> &TransactionId {
        &self.call.transaction
    }

    /// Get the function name
    pub fn function_name(&self) -> &str {
        &self.call.function
    }

    /// Get the timeout for this function call
    pub fn timeout(&self) -> u32 {
        self.call.timeout
    }

    /// Get the access level required for this call
    pub fn access(&self) -> Option<u32> {
        self.call.access
    }

    /// Get the source of this function call
    pub fn source(&self) -> Option<&str> {
        self.call.source.as_deref()
    }

    /// Get a parameter by name
    pub fn get_parameter(&self, name: &str) -> Option<&str> {
        self.parameters.get(name).map(|s| s.as_str())
    }

    /// Get all parameters
    pub fn parameters(&self) -> &HashMap<String, String> {
        &self.parameters
    }

    /// Check if a parameter exists
    pub fn has_parameter(&self, name: &str) -> bool {
        self.parameters.contains_key(name)
    }

    /// Get a parameter as a specific type
    pub fn get_parameter_as<T>(&self, name: &str) -> Option<T>
    where
        T: std::str::FromStr,
    {
        self.get_parameter(name)?.parse().ok()
    }

    /// Get a parameter with a default value
    pub fn get_parameter_or<'a>(&'a self, name: &str, default: &'a str) -> &'a str {
        self.get_parameter(name).unwrap_or(default)
    }

    /// Get a parameter as a specific type with a default value
    pub fn get_parameter_as_or<T>(&self, name: &str, default: T) -> T
    where
        T: std::str::FromStr,
    {
        self.get_parameter_as(name).unwrap_or(default)
    }

    /// Get function execution metadata
    pub fn metadata(&self) -> &FunctionMetadata {
        &self.metadata
    }

    /// Get the original function call
    pub fn call(&self) -> &FunctionCall {
        &self.call
    }

    /// Get elapsed time since function started
    pub fn elapsed(&self) -> std::time::Duration {
        self.metadata.start_time.elapsed().unwrap_or_default()
    }

    /// Check if this function call has timed out
    pub fn is_timed_out(&self) -> bool {
        self.elapsed().as_secs() > self.timeout() as u64
    }

    /// Parse parameters from the function call
    /// This is a simple implementation - could be enhanced to parse query strings, etc.
    fn parse_parameters(call: &FunctionCall) -> HashMap<String, String> {
        let mut parameters = HashMap::new();

        // Basic parameters from call metadata
        if let Some(ref source) = call.source {
            parameters.insert("source".to_string(), source.clone());
        }

        if let Some(access) = call.access {
            parameters.insert("access".to_string(), access.to_string());
        }

        parameters.insert("timeout".to_string(), call.timeout.to_string());
        parameters.insert("function".to_string(), call.function.clone());
        parameters.insert("transaction".to_string(), call.transaction.clone());

        // TODO: Parse actual query parameters from source or other fields
        // This could include parsing URL query strings, JSON payloads, etc.

        debug!("Parsed parameters for {}: {:?}", call.function, parameters);
        parameters
    }
}
