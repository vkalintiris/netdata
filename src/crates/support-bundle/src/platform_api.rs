//! The facts a platform's `bundle_facts` hands back to the shared
//! orchestration in main.rs.

use crate::summary::SummaryInputs;

pub struct BundleFacts {
    pub summary: SummaryInputs,
    pub agent_running: bool,
    pub api_ok: bool,
    pub is_container: bool,
    pub docker_logs_needed: bool,
}
