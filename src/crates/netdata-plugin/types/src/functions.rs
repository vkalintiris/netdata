use crate::HttpAccess;

/// A function declaration message for Netdata's external plugin protocol
#[derive(Debug, Clone)]
pub struct FunctionDeclaration {
    /// True if the function is global
    pub global: bool,
    /// The name of the function
    pub name: String,
    /// Timeout in seconds for function execution
    pub timeout: u32,
    /// Help text describing what the function does
    pub help: String,
    /// Tags of the function
    pub tags: Option<String>,
    /// Access control flags for the function
    pub access: Option<HttpAccess>,
    /// Priority level for function execution
    pub priority: Option<u32>,
    /// Version of the function
    pub version: Option<u32>,
}

/// A function call message for invoking functions
#[derive(Debug, Clone)]
pub struct FunctionCall {
    /// Transaction ID for this function call
    pub transaction: String,
    /// Timeout in seconds for function execution
    pub timeout: u32,
    /// Function name to call
    pub name: String,
    /// Access control flags for the function
    pub access: Option<HttpAccess>,
    /// Source information containing caller details
    pub source: Option<String>,
    /// Payload data for the function call (optional)
    pub payload: Option<Vec<u8>>,
}

/// A function result message containing the response payload
#[derive(Debug, Clone)]
pub struct FunctionResult {
    /// Transaction ID or unique identifier for this function call
    pub transaction: String,
    /// Status of the function call
    pub status: u32,
    /// Content type of the result (e.g., "application/json", "text/plain")
    pub format: String,
    /// Expires timestamp
    pub expires: u32,
    /// Result payload data
    pub payload: Vec<u8>,
}

/// A function cancel message for terminating function execution
#[derive(Debug, Clone)]
pub struct FunctionCancel {
    /// Transaction ID of the function call to cancel
    pub transaction: String,
}
