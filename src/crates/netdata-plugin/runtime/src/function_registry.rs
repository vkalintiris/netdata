use crate::{FunctionCall, FunctionContext, FunctionDeclaration, FunctionResult, PluginContext};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Type-erased async function handler
type FunctionHandler = Box<
    dyn Fn(PluginContext, FunctionContext) -> Pin<Box<dyn Future<Output = FunctionResult> + Send>>
        + Send
        + Sync,
>;

/// Registered function with declaration and handler
struct RegisteredFunction {
    declaration: FunctionDeclaration,
    handler: Arc<FunctionHandler>,
}

/// Function registry that maintains registered functions and their handlers
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
    pub async fn register<F, Fut>(&self, declaration: FunctionDeclaration, handler: F)
    where
        F: Fn(PluginContext, FunctionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = FunctionResult> + Send + 'static,
    {
        let function_name = declaration.name.clone();

        // Type-erase the handler
        let boxed_handler: FunctionHandler =
            Box::new(move |ctx, fn_ctx| Box::pin(handler(ctx, fn_ctx)));

        let registered = RegisteredFunction {
            declaration,
            handler: Arc::new(boxed_handler),
        };

        let mut functions = self.functions.write().await;
        functions.insert(function_name.clone(), registered);

        info!("Registered function: {}", function_name);
    }

    /// Call a function by name with the given context and cancellation token
    pub async fn call_function(
        &self,
        plugin_context: PluginContext,
        call: FunctionCall,
        cancellation_token: CancellationToken,
    ) -> Option<FunctionResult> {
        let function_name = &call.name;
        let functions = self.functions.read().await;

        if let Some(registered) = functions.get(function_name) {
            debug!(
                "Executing function: {} (transaction: {})",
                function_name, call.transaction
            );

            // Create function context with cancellation token
            let fn_context = FunctionContext::with_cancellation(call, cancellation_token);

            // Execute handler
            let result = (registered.handler)(plugin_context, fn_context).await;
            Some(result)
        } else {
            warn!("Function not found: {}", function_name);
            None
        }
    }

    /// Get all registered function declarations
    pub async fn get_all_declarations(&self) -> Vec<FunctionDeclaration> {
        let functions = self.functions.read().await;
        // filter out config-related functions
        functions
            .values()
            .filter(|f| !f.declaration.name.starts_with("config "))
            .map(|f| f.declaration.clone())
            .collect()
    }
}

impl Default for FunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
