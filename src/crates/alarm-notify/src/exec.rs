//! Subprocess helpers.
//!
//! Three senders keep delegating to the tools they always used - `sendmail`, the
//! `aws` CLI and smstools3's `sendsms` - because those own the user's MTA
//! configuration and cloud credentials, and reimplementing them would silently
//! change which identity a notification is sent as. Everything else is native.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Result of running an external program.
pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    /// Combined output, trimmed - used only in log messages.
    pub fn combined(&self) -> String {
        let mut s = String::new();
        s.push_str(self.stdout.trim());
        if !self.stderr.trim().is_empty() {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(self.stderr.trim());
        }
        s
    }
}

/// Locate an executable on `PATH`, honouring Windows' executable extensions.
pub fn which(program: &str) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.is_absolute() || program.contains('/') || program.contains('\\') {
        return p.is_file().then(|| p.to_path_buf());
    }

    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        vec![String::new()]
    };

    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{program}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// A POSIX shell, if this system has one.
///
/// Used for config-file command substitution and for the `custom_sender()`
/// compatibility shim. On Windows there may be none, and callers must cope.
pub fn posix_shell() -> Option<PathBuf> {
    for candidate in ["bash", "sh"] {
        if let Some(p) = which(candidate) {
            return Some(p);
        }
    }
    #[cfg(unix)]
    for fixed in ["/bin/bash", "/bin/sh"] {
        let p = Path::new(fixed);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    None
}

/// Run a program with arguments, optionally writing `stdin_data` to its input.
pub fn run<S, I>(
    program: &Path,
    args: I,
    stdin_data: Option<&[u8]>,
    env: &[(String, String)],
) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn()?;
    if let Some(data) = stdin_data {
        // The child may exit before consuming everything (a rejecting sendmail);
        // a short write is not our failure, the exit status is.
        if let Some(mut sink) = child.stdin.take() {
            let _ = sink.write_all(data);
        }
    }
    let out = child.wait_with_output()?;
    Ok(Output {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

#[cfg(test)]
#[path = "exec_tests.rs"]
mod tests;
