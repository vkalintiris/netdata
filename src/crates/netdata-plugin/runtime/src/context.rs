use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

pub type TransactionId = String;

/// Information about an ongoing function call
#[derive(Debug, Clone)]
pub struct Transaction {
    pub id: TransactionId,
    pub function_name: String,
    pub start_time: u64,
    pub timeout: u32,
    pub source: Option<String>,
    pub access: Option<u32>,
}

impl Transaction {
    pub fn new(
        id: TransactionId,
        function_name: String,
        timeout: u32,
        source: Option<String>,
        access: Option<u32>,
    ) -> Self {
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            id,
            function_name,
            start_time,
            timeout,
            source,
            access,
        }
    }

    pub fn elapsed(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.start_time)
    }

    pub fn is_expired(&self) -> bool {
        self.elapsed() > self.timeout as u64
    }
}

/// Plugin statistics and metrics
#[derive(Debug, Default, Clone)]
pub struct PluginStats {
    pub total_calls: u64,
    pub successful_calls: u64,
    pub failed_calls: u64,
    pub timed_out_calls: u64,
    pub active_transactions: u64,
}

/// Plugin context that maintains state and ongoing transactions
#[derive(Debug)]
pub struct PluginContext {
    plugin_name: String,
    transactions: Arc<RwLock<HashMap<TransactionId, Transaction>>>,
    stats: Arc<RwLock<PluginStats>>,
}

impl PluginContext {
    pub fn new(plugin_name: String) -> Self {
        Self {
            plugin_name,
            transactions: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(PluginStats::default())),
        }
    }

    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    /// Start a new transaction
    pub async fn start_transaction(&self, transaction: Transaction) {
        debug!("Starting transaction: {} for function: {}", transaction.id, transaction.function_name);
        
        let mut transactions = self.transactions.write().await;
        transactions.insert(transaction.id.clone(), transaction);

        // Update stats
        let mut stats = self.stats.write().await;
        stats.total_calls += 1;
        stats.active_transactions += 1;
    }

    /// Complete a transaction successfully
    pub async fn complete_transaction(&self, transaction_id: &TransactionId) {
        debug!("Completing transaction: {}", transaction_id);
        
        let mut transactions = self.transactions.write().await;
        transactions.remove(transaction_id);

        let mut stats = self.stats.write().await;
        stats.successful_calls += 1;
        stats.active_transactions = stats.active_transactions.saturating_sub(1);
    }

    /// Fail a transaction
    pub async fn fail_transaction(&self, transaction_id: &TransactionId) {
        debug!("Failing transaction: {}", transaction_id);
        
        let mut transactions = self.transactions.write().await;
        transactions.remove(transaction_id);

        let mut stats = self.stats.write().await;
        stats.failed_calls += 1;
        stats.active_transactions = stats.active_transactions.saturating_sub(1);
    }

    /// Cancel a transaction
    pub async fn cancel_transaction(&self, transaction_id: &TransactionId) {
        debug!("Cancelling transaction: {}", transaction_id);
        
        let mut transactions = self.transactions.write().await;
        transactions.remove(transaction_id);

        let mut stats = self.stats.write().await;
        stats.active_transactions = stats.active_transactions.saturating_sub(1);
    }

    /// Get information about a specific transaction
    pub async fn get_transaction(&self, transaction_id: &TransactionId) -> Option<Transaction> {
        let transactions = self.transactions.read().await;
        transactions.get(transaction_id).cloned()
    }

    /// Get all active transactions
    pub async fn get_active_transactions(&self) -> Vec<Transaction> {
        let transactions = self.transactions.read().await;
        transactions.values().cloned().collect()
    }

    /// Clean up expired transactions
    pub async fn cleanup_expired_transactions(&self) {
        let mut transactions = self.transactions.write().await;
        let expired: Vec<TransactionId> = transactions
            .values()
            .filter(|t| t.is_expired())
            .map(|t| t.id.clone())
            .collect();

        for id in expired {
            warn!("Transaction {} expired after {} seconds", id, transactions.get(&id).unwrap().elapsed());
            transactions.remove(&id);

            // Update stats
            let mut stats = self.stats.write().await;
            stats.timed_out_calls += 1;
            stats.active_transactions = stats.active_transactions.saturating_sub(1);
        }
    }

    /// Get current plugin statistics
    pub async fn get_stats(&self) -> PluginStats {
        let stats = self.stats.read().await;
        stats.clone()
    }

    /// Reset plugin statistics
    pub async fn reset_stats(&self) {
        let mut stats = self.stats.write().await;
        *stats = PluginStats::default();
    }
}