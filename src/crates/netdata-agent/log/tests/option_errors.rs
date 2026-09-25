//! An option error is logged when its option is reached, under the options before it: with `json` after the bad
//! option, the error is still written as logfmt (R4 M1).

use netdata_agent_log::{
    Source, initialize, set_default_log_dir, set_program_name, set_user_settings,
};

#[test]
fn an_option_error_is_written_before_the_later_options_apply() {
    let dir = std::env::temp_dir().join(format!("ndlog-option-errors-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let daemon = dir.join("daemon.log").to_string_lossy().into_owned();

    set_program_name("netdata");
    set_default_log_dir(&dir.to_string_lossy());
    for source in [Source::Collector, Source::Access, Source::Health] {
        set_user_settings(source, "none");
    }
    set_user_settings(Source::Daemon, &daemon);
    initialize();
    set_user_settings(Source::Daemon, &format!("level=info,bogus3,json@{daemon}"));

    let text = std::fs::read_to_string(&daemon).unwrap();
    let line = text.lines().last().unwrap();
    assert!(line.starts_with("time="), "not logfmt: {line}");
    assert!(
        line.ends_with(&format!(
            "msg=\"Error while parsing configuration of log source 'daemon'. In config 'level=info,bogus3,json@{daemon}', \
             'bogus3' is not understood.\""
        )),
        "{line}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
