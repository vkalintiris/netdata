use netdata_plugin_runtime::FunctionCall;
use std::collections::HashMap;
use tracing::debug;

/// Context passed to function handlers containing call information and utilities
#[derive(Debug, Clone)]
pub struct Context {
    call: FunctionCall,
    parameters: HashMap<String, String>,
}

impl Context {
    /// Create a new context from a function call
    pub fn new(call: FunctionCall) -> Self {
        let parameters = Self::parse_parameters(&call);
        
        Self {
            call,
            parameters,
        }
    }

    /// Get the underlying function call
    pub fn call(&self) -> &FunctionCall {
        &self.call
    }

    /// Get the transaction ID
    pub fn transaction_id(&self) -> &str {
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

    /// Parse parameters from the function call
    /// This is a simple implementation - in a real scenario, you might want to parse
    /// query parameters from the source or implement a more sophisticated parameter system
    fn parse_parameters(call: &FunctionCall) -> HashMap<String, String> {
        let mut parameters = HashMap::new();
        
        // For now, we'll just create some basic parameters from the call information
        if let Some(ref source) = call.source {
            parameters.insert("source".to_string(), source.clone());
        }
        
        if let Some(access) = call.access {
            parameters.insert("access".to_string(), access.to_string());
        }
        
        parameters.insert("timeout".to_string(), call.timeout.to_string());
        parameters.insert("function".to_string(), call.function.clone());
        parameters.insert("transaction".to_string(), call.transaction.clone());

        debug!("Parsed parameters: {:?}", parameters);
        parameters
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
}