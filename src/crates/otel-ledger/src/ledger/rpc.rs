//! Handlers for supervisor requests (function calls, shutdown).

use bridge::{LedgerRequest, LedgerResponse};

use super::Ledger;

impl Ledger {
    /// Handle a supervisor request. Returns `true` if the loop should exit.
    pub(super) async fn handle_supervisor_req(
        &mut self,
        req: LedgerRequest,
    ) -> Result<bool, ferryboat::Error> {
        match req {
            LedgerRequest::Call {
                transaction,
                name,
                args,
                ..
            } => {
                tracing::info!("function call: name={name} args={args:?}");
                let result = self.handle_function_call(&name, &args);
                let resp = LedgerResponse::Result(netdata_plugin_types::FunctionResult {
                    transaction,
                    ..result
                });
                self.supervisor.send(resp).await?;
                Ok(false)
            }
            LedgerRequest::Cancel { .. } => Ok(false),
            LedgerRequest::Shutdown => {
                tracing::info!("received Shutdown from supervisor");
                Ok(true)
            }
            LedgerRequest::Configure(_) => {
                tracing::warn!("unexpected late Configure message");
                Ok(false)
            }
        }
    }

    fn handle_function_call(
        &self,
        name: &str,
        args: &[String],
    ) -> netdata_plugin_types::FunctionResult {
        match name {
            "otel-logs" => {
                let mut total_wal = 0;
                let mut total_index = 0;
                for (_tenant_id, registry) in self.registries.tenants.iter() {
                    total_wal += registry.wal.len();
                    total_index += registry.sfst.len();
                }
                let payload = format!(
                    "otel-logs called with args: {args:?}\ntenants={} wal_files={total_wal} index_files={total_index}",
                    self.registries.tenants.len(),
                );
                netdata_plugin_types::FunctionResult {
                    transaction: String::new(),
                    status: 200,
                    format: "text/plain".to_string(),
                    expires: 0,
                    payload: payload.into_bytes(),
                }
            }
            _ => netdata_plugin_types::FunctionResult {
                transaction: String::new(),
                status: 404,
                format: "text/plain".to_string(),
                expires: 0,
                payload: format!("unknown function: {name}").into_bytes(),
            },
        }
    }
}
