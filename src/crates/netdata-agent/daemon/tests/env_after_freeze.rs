//! After the freeze (D134.4, D140) the log's exports reach the children's environment through the writer the daemon
//! installs, and never the process's own; before it, `env::get` reads the process's.

#[test]
fn a_log_export_after_the_freeze_reaches_the_children() {
    assert_eq!(netdata_agent_spawn::env::get("PATH"), std::env::var_os("PATH"));
    netdata_agent_log::set_env_writer(netdata_agent_spawn::env::set);
    netdata_agent_spawn::env::freeze();
    netdata_agent_log::set_priority_level("warning");
    assert_eq!(netdata_agent_spawn::env::get("NETDATA_LOG_LEVEL"), Some("warning".into()));
    assert!(netdata_agent_spawn::env::block().iter().any(|e| e.as_bytes() == b"NETDATA_LOG_LEVEL=warning"));
    assert_eq!(std::env::var_os("NETDATA_LOG_LEVEL"), None);
}
