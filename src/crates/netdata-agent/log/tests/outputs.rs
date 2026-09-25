//! Records written through configured file destinations. The logger is process-global, so this binary holds one
//! test; the collector and debug sources stay off because they would replace the test's own fd 2 and fd 1.

use netdata_agent_log::{
    Field, Priority, Source, Value, initialize, limits_reset, nd_log, netdata_log_error,
    netdata_log_info, push, set_default_log_dir, set_program_name, set_user_settings,
};

/// The line with its time and tid replaced, which differ between runs.
fn masked(line: &str) -> String {
    line.split(' ')
        .map(|word| {
            if word.starts_with("time=") {
                "time=T"
            } else if word.starts_with("tid=") {
                "tid=N"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn masked_json(line: &str) -> String {
    let mut out = String::new();
    for member in line.split(',') {
        if !out.is_empty() {
            out.push(',');
        }
        if member.contains("\"time\":") {
            out.push_str("{\"time\":T");
        } else if member.starts_with("\"tid\":") {
            out.push_str("\"tid\":N");
        } else {
            out.push_str(member);
        }
    }
    out
}

#[test]
fn records_reach_their_files_in_the_configured_format() {
    let dir = std::env::temp_dir().join(format!("ndlog-outputs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |name: &str| dir.join(name).to_string_lossy().into_owned();

    set_program_name("netdata");
    set_default_log_dir(&dir.to_string_lossy());
    set_user_settings(Source::Collector, "none");
    set_user_settings(
        Source::Daemon,
        &format!("protection=3/1m@{}", path("daemon.log")),
    );
    set_user_settings(Source::Access, &format!("json@{}", path("access.json")));
    set_user_settings(Source::Health, "off");
    initialize();
    limits_reset();

    // runs on a named thread, as the daemon's workers do
    std::thread::Builder::new()
        .name("WEB[3]".to_string())
        .spawn(|| {
            netdata_log_info!("first message");
            {
                let _request = push(vec![
                    (Field::SrcIp, Value::txt("localhost")),
                    (Field::SrcPort, Value::txt("50390")),
                    (Field::RequestMethod, Value::txt("GET")),
                    (Field::ResponseCode, Value::U64(200)),
                    (Field::Request, Value::txt("/api/v1/info")),
                ]);
                netdata_agent_log::logger(
                    Source::Access,
                    Priority::Info,
                    0,
                    &netdata_agent_log::here!(),
                    None,
                );
            }
            for i in 0..4 {
                netdata_log_error!(errno = 2; "error {i}");
            }
            nd_log!(Source::Health, Priority::Warning, "health is off");
        })
        .unwrap()
        .join()
        .unwrap();

    let daemon = std::fs::read_to_string(path("daemon.log")).unwrap();
    let daemon: Vec<String> = daemon.lines().map(masked).collect();
    assert_eq!(
        daemon,
        [
            "time=T comm=netdata source=daemon level=info tid=N thread=WEB[3] msg=\"first message\"",
            "time=T comm=netdata source=daemon level=error errno=\"2, No such file or directory\" tid=N \
             thread=WEB[3] msg=\"error 0\"",
            "time=T comm=netdata source=daemon level=error errno=\"2, No such file or directory\" tid=N \
             thread=WEB[3] msg=\"error 1\"",
            "time=T comm=netdata source=daemon msg_id=ec87a56120d5431bace51e2fb8bba243 msg=\"LOG FLOOD \
             PROTECTION: too many logs (4 logs in 0 seconds, threshold is set to 3 logs in 60 seconds). Preventing \
             more logs from process 'netdata' for 59 seconds.\"",
        ]
    );

    let access = std::fs::read_to_string(path("access.json")).unwrap();
    let access: Vec<String> = access.lines().map(masked_json).collect();
    assert_eq!(
        access,
        [
            "{\"time\":T,\"comm\":\"netdata\",\"source\":\"access\",\"level\":6,\"tid\":N,\"thread\":\"WEB[3]\",\
          \"src_ip\":\"localhost\",\"src_port\":\"50390\",\"req_method\":\"GET\",\"code\":200,\
          \"request\":\"/api/v1/info\"}"
        ]
    );

    // the aclk source keeps its compiled file, created although nothing is written to it
    assert_eq!(std::fs::read_to_string(path("aclk.log")).unwrap(), "");
    assert!(!dir.join("health.log").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
