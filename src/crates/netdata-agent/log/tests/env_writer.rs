//! The log's exports go through the writer its owner installs (D140): the daemon's frozen environment for its
//! children instead of the process's.

use std::sync::Mutex;

static WRITTEN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn record(key: &str, value: &str) -> std::io::Result<()> {
    WRITTEN.lock().unwrap().push((key.to_string(), value.to_string()));
    Ok(())
}

#[test]
fn exports_go_through_the_installed_writer() {
    netdata_agent_log::set_env_writer(record);
    netdata_agent_log::set_priority_level("err");
    assert_eq!(*WRITTEN.lock().unwrap(), [("NETDATA_LOG_LEVEL".to_string(), "error".to_string())]);
    assert_eq!(std::env::var_os("NETDATA_LOG_LEVEL"), None);
}
