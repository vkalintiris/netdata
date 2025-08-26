use crate::config_registry::{Config, ConfigRegistry};
use netdata_plugin_protocol::HttpAccess;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Transaction identifier type
pub type TransactionId = String;

/// Represents an active function call transaction
#[derive(Debug, Clone)]
pub struct Transaction {
    pub id: TransactionId,
    pub function_name: String,
    pub started_at: Instant,
    pub timeout: Duration,
    pub source: Option<String>,
    pub access: Option<HttpAccess>,
    pub cancelled: bool,
}

impl Transaction {
    /// Create a new transaction
    pub fn new(
        id: TransactionId,
        function_name: String,
        timeout_secs: u32,
        source: Option<String>,
        access: Option<HttpAccess>,
    ) -> Self {
        Self {
            id,
            function_name,
            started_at: Instant::now(),
            timeout: Duration::from_secs(timeout_secs as u64),
            source,
            access,
            cancelled: false,
        }
    }

    /// Check if the transaction has expired
    pub fn is_expired(&self) -> bool {
        self.started_at.elapsed() > self.timeout
    }

    /// Get elapsed time since transaction started
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

/// Plugin statistics
#[derive(Debug, Default, Clone)]
pub struct PluginStats {
    pub total_calls: u64,
    pub successful_calls: u64,
    pub failed_calls: u64,
    pub cancelled_calls: u64,
    pub timed_out_calls: u64,
    pub active_transactions: usize,
}

/// Transaction registry for managing active function calls
struct TransactionRegistry {
    transactions: HashMap<TransactionId, Transaction>,
}

impl TransactionRegistry {
    fn new() -> Self {
        Self {
            transactions: HashMap::new(),
        }
    }

    fn insert(&mut self, transaction: Transaction) {
        self.transactions
            .insert(transaction.id.clone(), transaction);
    }

    fn remove(&mut self, id: &TransactionId) -> Option<Transaction> {
        self.transactions.remove(id)
    }

    fn get(&self, id: &TransactionId) -> Option<&Transaction> {
        self.transactions.get(id)
    }

    fn get_mut(&mut self, id: &TransactionId) -> Option<&mut Transaction> {
        self.transactions.get_mut(id)
    }

    fn cleanup_expired(&mut self) -> Vec<TransactionId> {
        let expired: Vec<TransactionId> = self
            .transactions
            .values()
            .filter(|t| t.is_expired())
            .map(|t| t.id.clone())
            .collect();

        for id in &expired {
            self.transactions.remove(id);
        }

        expired
    }

    fn len(&self) -> usize {
        self.transactions.len()
    }

    fn values(&self) -> impl Iterator<Item = &Transaction> {
        self.transactions.values()
    }
}

/// Plugin context maintaining state and transaction registry
pub struct PluginContextInner {
    plugin_name: String,
    config_registry: RwLock<ConfigRegistry>,
    transaction_registry: RwLock<TransactionRegistry>,
    stats: RwLock<PluginStats>,
}

#[derive(Clone)]
pub struct PluginContext {
    inner: Arc<PluginContextInner>,
}

impl PluginContext {
    /// Create a new plugin context
    pub fn new(plugin_name: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(PluginContextInner {
                config_registry: RwLock::default(),
                plugin_name: plugin_name.into(),
                transaction_registry: RwLock::new(TransactionRegistry::new()),
                stats: RwLock::new(PluginStats::default()),
            }),
        }
    }

    pub async fn get_config(&self, id: &str) -> Option<Config> {
        let cfg_registry = self.inner.config_registry.read().await;
        cfg_registry.get(id).await
    }

    pub async fn insert_config(&self, cfg: Config) {
        let cfg_registry = self.inner.config_registry.read().await;
        cfg_registry.add(cfg).await
    }

    /// Get the plugin name
    pub fn plugin_name(&self) -> &str {
        &self.inner.plugin_name
    }

    /// Start a new transaction
    /// Returns false if a transaction with the same ID already exists
    pub async fn start_transaction(
        &self,
        id: TransactionId,
        function_name: String,
        timeout_secs: u32,
        source: Option<String>,
        access: Option<HttpAccess>,
    ) -> bool {
        let transaction = Transaction::new(
            id.clone(),
            function_name.clone(),
            timeout_secs,
            source,
            access,
        );

        debug!("Starting transaction {} for function {}", id, function_name);

        let mut registry = self.inner.transaction_registry.write().await;

        // Check if transaction already exists
        if registry.transactions.contains_key(&id) {
            warn!("Transaction {} already exists - rejecting duplicate", id);
            return false;
        }

        registry.insert(transaction);

        let mut stats = self.inner.stats.write().await;
        stats.total_calls += 1;
        stats.active_transactions = registry.len();

        true
    }

    /// Complete a transaction successfully
    pub async fn complete_transaction(&self, transaction_id: &TransactionId) {
        debug!("Completing transaction: {}", transaction_id);

        let mut registry = self.inner.transaction_registry.write().await;
        if let Some(transaction) = registry.remove(transaction_id) {
            info!(
                "Transaction {} completed successfully (elapsed: {:?})",
                transaction_id,
                transaction.elapsed()
            );

            let mut stats = self.inner.stats.write().await;
            stats.successful_calls += 1;
            stats.active_transactions = registry.len();
        }
    }

    /// Fail a transaction
    pub async fn fail_transaction(&self, transaction_id: &TransactionId) {
        debug!("Failing transaction: {}", transaction_id);

        let mut registry = self.inner.transaction_registry.write().await;
        if let Some(transaction) = registry.remove(transaction_id) {
            warn!(
                "Transaction {} failed (elapsed: {:?})",
                transaction_id,
                transaction.elapsed()
            );

            let mut stats = self.inner.stats.write().await;
            stats.failed_calls += 1;
            stats.active_transactions = registry.len();
        }
    }

    /// Cancel a transaction
    pub async fn cancel_transaction(&self, transaction_id: &TransactionId) {
        debug!("Cancelling transaction: {}", transaction_id);

        let mut registry = self.inner.transaction_registry.write().await;
        if let Some(transaction) = registry.get_mut(transaction_id) {
            transaction.cancelled = true;
            info!(
                "Transaction {} cancelled (elapsed: {:?})",
                transaction_id,
                transaction.elapsed()
            );

            // Note: We don't remove it immediately as the handler might still be running
            // It will be cleaned up on completion or during cleanup_expired_transactions

            let mut stats = self.inner.stats.write().await;
            stats.cancelled_calls += 1;
        }
    }

    /// Check if a transaction is cancelled
    pub async fn is_transaction_cancelled(&self, transaction_id: &TransactionId) -> bool {
        let registry = self.inner.transaction_registry.read().await;
        registry
            .get(transaction_id)
            .map(|t| t.cancelled)
            .unwrap_or(false)
    }

    /// Get transaction details
    pub async fn get_transaction(&self, transaction_id: &TransactionId) -> Option<Transaction> {
        let registry = self.inner.transaction_registry.read().await;
        registry.get(transaction_id).cloned()
    }

    /// Get all active transactions
    pub async fn get_active_transactions(&self) -> Vec<Transaction> {
        let registry = self.inner.transaction_registry.read().await;
        registry.values().cloned().collect()
    }

    /// Clean up expired transactions
    pub async fn cleanup_expired_transactions(&self) {
        let mut registry = self.inner.transaction_registry.write().await;
        let expired = registry.cleanup_expired();

        if !expired.is_empty() {
            let mut stats = self.inner.stats.write().await;
            stats.timed_out_calls += expired.len() as u64;
            stats.active_transactions = registry.len();

            for id in expired {
                warn!("Transaction {} timed out", id);
            }
        }
    }

    /// Get current plugin statistics
    pub async fn get_stats(&self) -> PluginStats {
        let stats = self.inner.stats.read().await;
        stats.clone()
    }

    /// Reset plugin statistics
    pub async fn reset_stats(&self) {
        let mut stats = self.inner.stats.write().await;
        let registry = self.inner.transaction_registry.read().await;
        *stats = PluginStats {
            active_transactions: registry.len(),
            ..Default::default()
        };
    }
}
