use serde_json::Value;

/// Response types that function handlers can return
#[derive(Debug, Clone)]
pub enum FunctionResponse {
    /// Plain text response
    Text(String),
    
    /// JSON response
    Json(Value),
    
    /// Custom response with specific content type and status
    Custom {
        status: u32,
        content_type: String,
        data: Vec<u8>,
    },
}

impl FunctionResponse {
    /// Create a text response
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text(content.into())
    }

    /// Create a JSON response
    pub fn json(value: Value) -> Self {
        Self::Json(value)
    }

    /// Create a custom response
    pub fn custom(status: u32, content_type: impl Into<String>, data: Vec<u8>) -> Self {
        Self::Custom {
            status,
            content_type: content_type.into(),
            data,
        }
    }

    /// Create an HTML response
    pub fn html(content: impl Into<String>) -> Self {
        let content = content.into();
        Self::Custom {
            status: 200,
            content_type: "text/html".to_string(),
            data: content.into_bytes(),
        }
    }

    /// Create a CSV response
    pub fn csv(content: impl Into<String>) -> Self {
        let content = content.into();
        Self::Custom {
            status: 200,
            content_type: "text/csv".to_string(),
            data: content.into_bytes(),
        }
    }

    /// Create an XML response
    pub fn xml(content: impl Into<String>) -> Self {
        let content = content.into();
        Self::Custom {
            status: 200,
            content_type: "application/xml".to_string(),
            data: content.into_bytes(),
        }
    }

    /// Create a binary response
    pub fn binary(content_type: impl Into<String>, data: Vec<u8>) -> Self {
        Self::Custom {
            status: 200,
            content_type: content_type.into(),
            data,
        }
    }

    /// Create an error response
    pub fn error(status: u32, message: impl Into<String>) -> Self {
        Self::Custom {
            status,
            content_type: "text/plain".to_string(),
            data: message.into().into_bytes(),
        }
    }

    /// Create a 404 Not Found response
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::error(404, message)
    }

    /// Create a 400 Bad Request response
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::error(400, message)
    }

    /// Create a 401 Unauthorized response
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::error(401, message)
    }

    /// Create a 403 Forbidden response
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::error(403, message)
    }

    /// Create a 500 Internal Server Error response
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::error(500, message)
    }
}

impl From<String> for FunctionResponse {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for FunctionResponse {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

impl From<Value> for FunctionResponse {
    fn from(json: Value) -> Self {
        Self::Json(json)
    }
}