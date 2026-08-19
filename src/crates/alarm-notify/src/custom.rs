//! The `custom` notification method.
//!
//! Historically this was a bash function, `custom_sender()`, written by the user
//! *inside* `health_alarm_notify.conf` and executed in the script's own scope. That
//! cannot be reimplemented in Rust, so it is preserved instead of dropped:
//!
//! 1. `CUSTOM_SENDER_COMMAND` - any executable, recipients as arguments and every
//!    notification variable in its environment. Portable, works on Windows, and the
//!    recommended way going forward (the same "exec handler" shape Alertmanager,
//!    Zabbix and Sensu use).
//! 2. An existing `custom_sender()` function - the shipped `custom-sender.sh` shim
//!    re-sources the user's configuration, restores the helpers the function expects
//!    (`urlencode`, `docurl`, `info`, ...) and calls it. Existing installations keep
//!    working with no edit.
//! 3. Windows - the shipped `custom-sender.ps1` shim calls a `Custom-Sender`
//!    function from `health_alarm_notify_custom.ps1`, since there is no bash.

use std::path::PathBuf;

use crate::config::Config;
use crate::exec;
use crate::paths::Paths;

pub enum CustomSender {
    /// A user-supplied executable.
    Command { program: PathBuf },
    /// The bash shim wrapping a `custom_sender()` function.
    ShellShim { shell: PathBuf, shim: PathBuf },
    /// The PowerShell shim wrapping a `Custom-Sender` function.
    PowerShellShim { powershell: PathBuf, shim: PathBuf },
}

impl CustomSender {
    pub fn describe(&self) -> String {
        match self {
            CustomSender::Command { program } => format!("command {}", program.display()),
            CustomSender::ShellShim { shim, .. } => {
                format!("custom_sender() via {}", shim.display())
            }
            CustomSender::PowerShellShim { shim, .. } => {
                format!("Custom-Sender via {}", shim.display())
            }
        }
    }
}

/// Directory holding the shipped shims - the same directory as this binary.
pub fn shim_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("NETDATA_PLUGINS_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
}

/// Decide how `custom` notifications will be delivered, if at all.
pub fn resolve_sender(cfg: &Config) -> Option<CustomSender> {
    let configured = cfg.str("CUSTOM_SENDER_COMMAND");
    if !configured.is_empty() {
        match exec::which(configured) {
            Some(program) => return Some(CustomSender::Command { program }),
            None => tracing::error!(
                "CUSTOM_SENDER_COMMAND '{configured}' was not found or is not executable"
            ),
        }
    }

    let dir = shim_dir();

    if cfg.data.custom_sender_body.is_some() {
        if let (Some(shell), Some(shim)) = (
            exec::posix_shell(),
            dir.as_ref().map(|d| d.join("custom-sender.sh")),
        ) {
            if shim.is_file() {
                return Some(CustomSender::ShellShim { shell, shim });
            }
        }
    }

    let paths = Paths::from_environment();
    let ps_user_file = paths.user_config_dir.join("health_alarm_notify_custom.ps1");
    if ps_user_file.is_file() {
        let powershell = exec::which("pwsh").or_else(|| exec::which("powershell"));
        if let (Some(powershell), Some(shim)) = (
            powershell,
            dir.as_ref().map(|d| d.join("custom-sender.ps1")),
        ) {
            if shim.is_file() {
                return Some(CustomSender::PowerShellShim { powershell, shim });
            }
        }
    }

    None
}

/// Run the resolved sender for `recipients`.
///
/// `env` carries every notification variable under the names the shell function
/// documentation promises (`host`, `status`, `alarm`, ...), so an existing
/// `custom_sender()` body sees exactly the scope it always did.
pub fn dispatch(sender: &CustomSender, recipients: &str, env: &[(String, String)]) -> bool {
    let result = match sender {
        CustomSender::Command { program } => exec::run(program, [recipients], None, env),
        CustomSender::ShellShim { shell, shim } => exec::run(
            shell,
            [shim.as_os_str(), std::ffi::OsStr::new(recipients)],
            None,
            env,
        ),
        CustomSender::PowerShellShim { powershell, shim } => exec::run(
            powershell,
            [
                std::ffi::OsStr::new("-NoProfile"),
                std::ffi::OsStr::new("-NonInteractive"),
                std::ffi::OsStr::new("-ExecutionPolicy"),
                std::ffi::OsStr::new("Bypass"),
                std::ffi::OsStr::new("-File"),
                shim.as_os_str(),
                std::ffi::OsStr::new(recipients),
            ],
            None,
            env,
        ),
    };

    match result {
        Ok(out) => {
            // The shim relays the function's own diagnostics on stderr; surface them
            // so a failing custom sender is diagnosable.
            for line in out.stderr.lines().filter(|l| !l.trim().is_empty()) {
                tracing::info!("custom sender: {line}");
            }
            if out.success() {
                true
            } else {
                tracing::error!(
                    "custom notification sender failed with exit code {:?}",
                    out.status
                );
                false
            }
        }
        Err(e) => {
            tracing::error!("could not run the custom notification sender: {e}");
            false
        }
    }
}
