//! Bundle contract constants. These mirror the values documented in
//! packaging/installer/SUPPORT-BUNDLE.md — change them only with the doc.

pub const TOOL_VERSION: &str = "1.0.0";
pub const SCHEMA: &str = "netdata-support-bundle/v1";

/// Hard wall-clock budget for the whole collection; checked before each
/// collector, so the hard runtime bound is deadline + one command timeout.
pub const GLOBAL_DEADLINE_SECS: u64 = 240;

/// Per-log-file size cap (line-aligned).
pub const LOG_CAP: u64 = 5 * 1024 * 1024;
/// Per-config/state-file size cap (line-aligned).
pub const FILE_CAP: u64 = 1024 * 1024;
/// Per-command/API-response cap; oversized JSON is withheld whole.
pub const API_CAP: u64 = 2 * 1024 * 1024;
/// Per-file cap for user config subtrees and dyncfg files.
pub const CONF_FILE_CAP: u64 = 262144;

pub const ND_PORT: u16 = 19999;
pub const CLOUD_HOST: &str = "app.netdata.cloud";

// Bundle paths read back by summary.rs after collection. Declared once so
// the item declarations and the summary readers cannot drift apart.
pub const PATH_INFO_V1: &str = "07-runtime/info-v1.json";
pub const PATH_ACLK_STATE: &str = "07-runtime/aclk-state.json";
pub const PATH_ACLK: &str = "07-runtime/aclk.json";
pub const PATH_CLOUD_STATE: &str = "06-state/cloud-state.txt";
pub const PATH_ERROR_LOG: &str = "05-logs/error.log";
pub const PATH_STATUS_FILE: &str = "06-state/status-file.json";
