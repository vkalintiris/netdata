//! Localhost's system info, ported from the startup step `system info` of `netdata_main()`:
//! `rrdhost_system_info_detect()` (`src/database/rrdhost-system-info.c`), `get_install_type()` and the second detection
//! of `populate_system_info()` (`src/daemon/buildinfo.c`). Decisions D49 in the status repository.

use std::io::Read;

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error};
use netdata_agent_rrd::system_info::SystemInfo;
use netdata_agent_text::c::fgets_chunks;

use crate::spawn::Popen;

/// `rrdhost_system_info_detect()`: `<plugins>/system-info.sh`'s pairs, each exported to the environment the
/// plugins inherit. The script's exit code does not matter.
fn detect(si: &mut SystemInfo, plugins_dir: &str) {
    // the daemon status file's hardware fields come with its port (D49 point 3)
    let script = format!("{plugins_dir}/system-info.sh");
    if nix::unistd::access(script.as_str(), nix::unistd::AccessFlags::R_OK).is_err() {
        netdata_log_error!("SYSTEM INFO: System info script {script} not found or not readable.");
        return;
    }
    let Ok(mut child) = Popen::run(&script) else {
        netdata_log_error!("SYSTEM INFO: Failed to execute system info script {script}.");
        return;
    };
    let mut stdout = Vec::new();
    let _ = child.stdout().read_to_end(&mut stdout);
    for (name, value) in si.apply_script_output(&stdout) {
        // nd_setenv(); the daemon is still single-threaded here
        if let Err(err) = netdata_agent_sys::setenv(&name, &value) {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "SYSTEM INFO: cannot export '{name}': {err}"
            );
        }
    }
    let _ = child.wait();
}

/// `get_value_from_key()`: what follows `KEY='`, without the quotes around it.
fn install_type_value(line: &[u8], key: &str) -> String {
    let mut value = &line[key.len() + 2..];
    while let [b'\'', rest @ ..] = value {
        value = rest;
    }
    while value.len() > 1 && value.last() == Some(&b'\'') {
        value = &value[..value.len() - 1];
    }
    String::from_utf8_lossy(value).into_owned()
}

/// `get_install_type()`: `<user config>/.install-type`, as the installers write it.
fn install_type(si: &mut SystemInfo, user_config_dir: &str) {
    let Ok(content) = std::fs::read(format!("{user_config_dir}/.install-type")) else {
        return;
    };
    for chunk in fgets_chunks(&content, 256) {
        // fgets_trim_len(): trailing newlines only
        let mut line = netdata_agent_text::c::c_str(chunk);
        while line.len() > 1 && line.last() == Some(&b'\n') {
            line = &line[..line.len() - 1];
        }
        for (key, field) in [
            ("INSTALL_TYPE", &mut si.install_type),
            ("PREBUILT_ARCH", &mut si.prebuilt_arch),
            ("PREBUILT_DISTRO", &mut si.prebuilt_dist),
        ] {
            if line.starts_with(key.as_bytes()) && line.get(key.len()..key.len() + 2) == Some(b"='")
            {
                *field = Some(install_type_value(line, key));
            }
        }
    }
}

/// The startup step `system info`: detection, the install type, then the second detection C runs for its build info
/// (`set_late_analytics_variables()` → `populate_system_info()`, localhost not existing yet), whose result only
/// analytics use.
pub fn startup(plugins_dir: &str, user_config_dir: &str) -> SystemInfo {
    let mut si = SystemInfo::default();
    detect(&mut si, plugins_dir);
    install_type(&mut si, user_config_dir);
    detect(&mut SystemInfo::default(), plugins_dir);
    si
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_type_lines_as_c_reads_them() {
        let dir = std::env::temp_dir().join(format!("netdata-install-type-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".install-type"),
            "# comment\nINSTALL_TYPE='kickstart-static'\nPREBUILT_ARCH=''x86_64''\n\nPREBUILT_DISTRO='debian\n",
        )
        .unwrap();
        let mut si = SystemInfo::default();
        install_type(&mut si, dir.to_str().unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(si.install_type.as_deref(), Some("kickstart-static"));
        assert_eq!(si.prebuilt_arch.as_deref(), Some("x86_64"));
        assert_eq!(si.prebuilt_dist.as_deref(), Some("debian"));
    }
}
