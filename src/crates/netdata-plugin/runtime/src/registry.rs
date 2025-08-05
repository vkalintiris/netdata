use crate::{FunctionCall, FunctionContext, FunctionDeclaration, FunctionResult, PluginContext};

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Type alias for function handler that receives both plugin context and function context
pub type FunctionHandler = Box<
    dyn Fn(
            Arc<PluginContext>,
            FunctionContext,
        ) -> Pin<Box<dyn Future<Output = FunctionResult> + Send>>
        + Send
        + Sync,
>;

/// Function metadata and handler
#[derive(Clone)]
pub struct RegisteredFunction {
    pub metadata: FunctionDeclaration,
    pub handler: Arc<FunctionHandler>,
}

/// Function registry that maintains a mapping of function names to handlers
#[derive(Clone)]
pub struct FunctionRegistry {
    functions: Arc<RwLock<HashMap<String, RegisteredFunction>>>,
}

impl FunctionRegistry {
    /// Create a new function registry
    pub fn new() -> Self {
        Self {
            functions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a function with its handler
    pub async fn register<F, Fut>(&self, function: FunctionDeclaration, handler: F)
    where
        F: Fn(Arc<PluginContext>, FunctionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = FunctionResult> + Send + 'static,
    {
        let function_name = function.name.clone();

        let boxed_handler: FunctionHandler =
            Box::new(move |plugin_ctx, fn_ctx| Box::pin(handler(plugin_ctx, fn_ctx)));

        let registered = RegisteredFunction {
            metadata: function,
            handler: Arc::new(boxed_handler),
        };

        let mut functions = self.functions.write().await;
        functions.insert(function_name.clone(), registered);

        debug!("Registered function: {}", function_name);
    }

    /// Unregister a function
    pub async fn unregister(&self, function_name: &str) -> bool {
        let mut functions = self.functions.write().await;
        let removed = functions.remove(function_name).is_some();

        if removed {
            debug!("Unregistered function: {}", function_name);
        } else {
            warn!(
                "Attempted to unregister non-existent function: {}",
                function_name
            );
        }

        removed
    }

    /// Get a function by name
    pub async fn get(&self, function_name: &str) -> Option<RegisteredFunction> {
        let functions = self.functions.read().await;
        functions.get(function_name).cloned()
    }

    /// Check if a function is registered
    pub async fn contains(&self, function_name: &str) -> bool {
        let functions = self.functions.read().await;
        functions.contains_key(function_name)
    }

    /// Get all registered function names
    pub async fn get_function_names(&self) -> Vec<String> {
        let functions = self.functions.read().await;
        functions.keys().cloned().collect()
    }

    /// Get all registered functions (metadata only)
    pub async fn get_all_functions(&self) -> Vec<FunctionDeclaration> {
        let functions = self.functions.read().await;
        functions.values().map(|f| f.metadata.clone()).collect()
    }

    /// Call a function by name
    pub async fn call_function(
        &self,
        plugin_context: Arc<PluginContext>,
        call: FunctionCall,
    ) -> Option<FunctionResult> {
        let function_name = call.function.clone();

        if let Some(registered_function) = self.get(&function_name).await {
            debug!(
                "Calling function: {} (transaction: {})",
                function_name, call.transaction
            );

            // Create function context from the call
            let function_context = FunctionContext::new(call);

            // Call handler with both contexts
            let result = (registered_function.handler)(plugin_context, function_context).await;
            Some(result)
        } else {
            warn!("Function not found: {}", function_name);
            None
        }
    }

    /// Get the number of registered functions
    pub async fn len(&self) -> usize {
        let functions = self.functions.read().await;
        functions.len()
    }

    /// Check if the registry is empty
    pub async fn is_empty(&self) -> bool {
        let functions = self.functions.read().await;
        functions.is_empty()
    }

    /// Clear all registered functions
    pub async fn clear(&self) {
        let mut functions = self.functions.write().await;
        functions.clear();
        debug!("Cleared all registered functions");
    }
}

impl Default for FunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
