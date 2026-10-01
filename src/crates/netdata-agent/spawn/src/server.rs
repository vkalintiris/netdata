//! The spawn server, the helper process (`spawn_server_nofork.c`). For now the records it writes when it reaps a
//! child, which the daemon's own popen shares until the server is wired in (D136.5).

use netdata_agent_log::{Priority, Source, nd_log};

/// `spawn_server_sigchld_handler()`'s records for a reaped child (`spawn_server_nofork.c:1166-1208`), with C's texts
/// and `[request not found]` without a command line: whether it ended (exited or killed), so its request is answered
/// and removed.
pub fn log_reaped(pid: i32, request_id: usize, raw: i32, cmdline: Option<&str>) -> bool {
    let cmd = cmdline.unwrap_or("[request not found]");
    if libc::WIFEXITED(raw) {
        let code = libc::WEXITSTATUS(raw);
        if code != 0 {
            nd_log!(
                Source::Collector,
                Priority::Warning,
                "SPAWN SERVER: child with pid {pid} (request {request_id}) exited with exit code {code}: {cmd}"
            );
        }
        true
    } else if libc::WIFSIGNALED(raw) {
        // SIGPIPE and SIGTERM are how children are stopped on purpose: not warnings
        let sig = libc::WTERMSIG(raw);
        if libc::WCOREDUMP(raw) {
            nd_log!(
                Source::Collector,
                Priority::Warning,
                "SPAWN SERVER: child with pid {pid} (request {request_id}) coredump'd due to signal {sig}: {cmd}"
            );
        } else {
            let priority = if matches!(sig, libc::SIGPIPE | libc::SIGTERM) { Priority::Debug } else { Priority::Warning };
            nd_log!(
                Source::Collector,
                priority,
                "SPAWN SERVER: child with pid {pid} (request {request_id}) killed by signal {sig}: {cmd}"
            );
        }
        true
    } else if libc::WIFSTOPPED(raw) {
        let sig = libc::WSTOPSIG(raw);
        nd_log!(
            Source::Collector,
            Priority::Warning,
            "SPAWN SERVER: child with pid {pid} (request {request_id}) stopped due to signal {sig}: {cmd}"
        );
        false
    } else if libc::WIFCONTINUED(raw) {
        nd_log!(
            Source::Collector,
            Priority::Warning,
            "SPAWN SERVER: child with pid {pid} (request {request_id}) continued due to signal {}: {cmd}",
            libc::SIGCONT
        );
        false
    } else {
        nd_log!(
            Source::Collector,
            Priority::Warning,
            "SPAWN SERVER: child with pid {pid} (request {request_id}) reports unhandled status: {cmd}"
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The records and the answer per kind of end, as C's handler writes them.
    #[test]
    fn reaped_children_are_logged_as_c() {
        let cases = [
            (0x300, Some((Priority::Warning, "exited with exit code 3: cmd")), true),
            (0, None, true),
            (libc::SIGTERM, Some((Priority::Debug, "killed by signal 15: cmd")), true),
            (libc::SIGKILL, Some((Priority::Warning, "killed by signal 9: cmd")), true),
            (libc::SIGSEGV | 0x80, Some((Priority::Warning, "coredump'd due to signal 11: cmd")), true),
            (libc::SIGSTOP << 8 | 0x7f, Some((Priority::Warning, "stopped due to signal 19: cmd")), false),
            (0xffff, Some((Priority::Warning, "continued due to signal 18: cmd")), false),
        ];
        for (raw, record, ended) in cases {
            let (got, records) = netdata_agent_log::capture(|| log_reaped(42, 7, raw, Some("cmd")));
            let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap())).collect();
            let want: Vec<_> = record
                .map(|(p, tail)| (Source::Collector, p, format!("SPAWN SERVER: child with pid 42 (request 7) {tail}")))
                .into_iter()
                .collect();
            assert_eq!((got, records), (ended, want), "{raw:#x}");
        }
        let (_, records) = netdata_agent_log::capture(|| log_reaped(42, 0, 0x100, None));
        assert!(records[0].message.as_deref().unwrap().ends_with("exit code 1: [request not found]"));
    }
}
