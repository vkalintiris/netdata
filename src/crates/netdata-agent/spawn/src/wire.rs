//! The spawn server's wire (`spawn_server_nofork.c`, `spawn_server_internals.h`, `spawn_library.c`, `spawn_popen.c`):
//! the request header and its blobs, the 12-byte reports, and what C derives from them, byte for byte as C's own code
//! produces them (the vectors of `evidence/m8-commit1/` in the status repository, D136). Native byte order: every
//! target the agent builds for is little-endian.

/// `SPAWN_SERVER_TRANSFER_FDS`: a request passes the child's stdin, stdout, stderr and a custom descriptor.
pub const TRANSFER_FDS: usize = 4;
/// `STATUS_REPORT_MAGIC`.
pub const REPORT_MAGIC: u32 = 0xBADA_55EE;
/// `SPAWN_SERVER_KILL_DEFAULT_GRACE_MS`.
pub const KILL_DEFAULT_GRACE_MS: i32 = 2000;
/// `MAX_IOV`: a request's `sendmsg()` vector.
pub const IOV_MAX: usize = 10;
/// `SPAWN_SERVER_MSG_*`.
pub const MSG_INVALID: u8 = 0;
pub const MSG_REQUEST: u8 = 1;
pub const MSG_PING: u8 = 2;
/// `SPAWN_INSTANCE_TYPE_*`.
pub const TYPE_EXEC: u8 = 0;
pub const TYPE_CALLBACK: u8 = 1;

/// The header's length when its four sizes are `width` bytes (`size_t`): 50 on 64-bit, 34 on 32-bit.
const fn header_len(width: usize) -> usize {
    2 + 16 + 4 * width
}

/// A request's header on this target.
pub const HEADER_LEN: usize = header_len(size_of::<usize>());

/// A request's header (`spawn_server_nofork.c:873-892`): the message type, the server's magic, the request's id, the
/// lengths of its environment, arguments and callback data, and the instance type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub msg_type: u8,
    pub magic: [u8; 16],
    pub request_id: usize,
    pub env_size: usize,
    pub argv_size: usize,
    pub data_size: usize,
    pub instance_type: u8,
}

impl Header {
    /// A PING: the type, everything else zero (`spawn_server_nofork.c:809`).
    pub fn ping() -> Header {
        Header {
            msg_type: MSG_PING,
            ..Header::default()
        }
    }

    /// The header's bytes on this target.
    pub fn encode(&self) -> Vec<u8> {
        self.encode_width(size_of::<usize>())
    }

    /// The header's bytes with `width`-byte sizes: the 32-bit layout can be checked on a 64-bit host.
    fn encode_width(&self, width: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(header_len(width));
        out.push(self.msg_type);
        out.extend_from_slice(&self.magic);
        for size in [self.request_id, self.env_size, self.argv_size, self.data_size] {
            out.extend_from_slice(&size.to_le_bytes()[..width]);
        }
        out.push(self.instance_type);
        out
    }

    /// A header read from exactly [`HEADER_LEN`] bytes.
    pub fn decode(b: &[u8]) -> Option<Header> {
        Header::decode_width(b, size_of::<usize>())
    }

    fn decode_width(b: &[u8], width: usize) -> Option<Header> {
        if b.len() != header_len(width) {
            return None;
        }
        let size = |i: usize| {
            let at = 17 + i * width;
            let mut bytes = [0u8; size_of::<usize>()];
            bytes[..width].copy_from_slice(&b[at..at + width]);
            usize::from_le_bytes(bytes)
        };
        Some(Header {
            msg_type: b[0],
            magic: b[1..17].try_into().ok()?,
            request_id: size(0),
            env_size: size(1),
            argv_size: size(2),
            data_size: size(3),
            instance_type: b[17 + 4 * width],
        })
    }
}

/// `STATUS_REPORT` (`spawn_server_nofork.c:176-182`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Status {
    None = 0,
    Started = 1,
    Failed = 2,
    Exited = 3,
    Ping = 4,
}

/// A report on a request's socket or the status pipe (`spawn_server_nofork.c:186-202`), as raw as C reads it: the
/// magic, the status, and the pid, the errno or the raw `waitpid()` status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub magic: u32,
    pub status: i32,
    pub value: i32,
}

impl Report {
    pub const LEN: usize = 12;

    fn new(status: Status, value: i32) -> Report {
        Report {
            magic: REPORT_MAGIC,
            status: status as i32,
            value,
        }
    }

    pub fn started(pid: i32) -> Report {
        Report::new(Status::Started, pid)
    }

    pub fn failed(errno: i32) -> Report {
        Report::new(Status::Failed, errno)
    }

    pub fn exited(raw: i32) -> Report {
        Report::new(Status::Exited, raw)
    }

    pub fn ping() -> Report {
        Report::new(Status::Ping, 0)
    }

    /// The server's first word on the status pipe: STARTED with its pid and no magic (`spawn_server_nofork.c:1268-1273`).
    pub fn handshake(pid: i32) -> Report {
        Report {
            magic: 0,
            ..Report::started(pid)
        }
    }

    pub fn encode(&self) -> [u8; 12] {
        let mut out = [0u8; 12];
        out[..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4..8].copy_from_slice(&self.status.to_le_bytes());
        out[8..].copy_from_slice(&self.value.to_le_bytes());
        out
    }

    pub fn decode(b: &[u8; 12]) -> Report {
        let word = |i: usize| [b[i], b[i + 1], b[i + 2], b[i + 3]];
        Report {
            magic: u32::from_le_bytes(word(0)),
            status: i32::from_le_bytes(word(4)),
            value: i32::from_le_bytes(word(8)),
        }
    }
}

/// `argv_encode()` (`spawn_server_nofork.c:94-125`): each non-empty string with its NUL, then one more NUL.
pub fn encode_list<S: AsRef<[u8]>>(items: &[S]) -> Vec<u8> {
    let mut out = Vec::new();
    for item in items.iter().map(AsRef::as_ref).filter(|i| !i.is_empty()) {
        out.extend_from_slice(item);
        out.push(0);
    }
    out.push(0);
    out
}

/// `argv_decode()` (`spawn_server_nofork.c:132-171`): the strings of a blob that ends in exactly one empty string, else
/// none.
pub fn decode_list(b: &[u8]) -> Option<Vec<&[u8]>> {
    if b.last() != Some(&0) {
        return None;
    }
    let mut items = Vec::new();
    let mut at = 0;
    while at < b.len() {
        if b[at] == 0 {
            return (at == b.len() - 1).then_some(items);
        }
        let end = at + b[at..].iter().position(|&c| c == 0)?;
        items.push(&b[at..end]);
        at = end + 1;
    }
    None
}

/// `argv_to_cmdline_buffer()` (`spawn_library.c:6-52`): the arguments joined by spaces; one with a blank or a `"` is
/// quoted after the first argument (only its closing quote before it) and its `"` escaped.
pub fn cmdline<S: AsRef<[u8]>>(argv: &[S]) -> String {
    let mut out = Vec::new();
    for arg in argv.iter().map(AsRef::as_ref) {
        let quote = arg.iter().any(|c| matches!(c, b' ' | 0x0b | b'\t' | b'\n' | b'"'));
        if quote && !out.is_empty() {
            out.extend_from_slice(b" \"");
        } else if !out.is_empty() {
            out.push(b' ');
        }
        for &c in arg {
            if c == b'"' {
                out.push(b'\\');
            }
            out.push(c);
        }
        if quote {
            out.push(b'"');
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `int` `waitpid()` fills, rebuilt from nix's reading of it: `exit << 8`, the signal with the core bit 0x80,
/// `signal << 8 | 0x7f` stopped, `0xffff` continued.
pub fn raw_wait_status(status: nix::sys::wait::WaitStatus) -> i32 {
    use nix::sys::wait::WaitStatus;
    match status {
        WaitStatus::Exited(_, code) => (code & 0xff) << 8,
        WaitStatus::Signaled(_, signal, core) => signal as i32 | if core { 0x80 } else { 0 },
        WaitStatus::Stopped(_, signal) => (signal as i32) << 8 | 0x7f,
        WaitStatus::Continued(_) => 0xffff,
        _ => 0,
    }
}

/// `spawn_popen_status_rc()` (`spawn_popen.c:144-161`): the exit code; 0 when killed by SIGTERM or SIGPIPE (how
/// children are stopped on purpose); else -1.
pub fn status_rc(raw: i32) -> i32 {
    if libc::WIFEXITED(raw) {
        libc::WEXITSTATUS(raw)
    } else if libc::WIFSIGNALED(raw) {
        match libc::WTERMSIG(raw) {
            libc::SIGTERM | libc::SIGPIPE => 0,
            _ => -1,
        }
    } else {
        -1
    }
}

/// `CMSG_LEN(sizeof(int) * fds)` in safe Rust (libc's is `const unsafe fn`): the aligned `cmsghdr` (a `size_t` and two
/// `int`s) and the descriptors: 32 for four on 64-bit, 28 on 32-bit.
pub const fn cmsg_len(fds: usize) -> usize {
    let width = size_of::<usize>();
    (width + 8).next_multiple_of(width) + 4 * fds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    /// C's own header for request 7 (magic 00..0f, env {A=1, B=2}, argv {/bin/sh, -c, echo x}) at both widths, and
    /// the PING's.
    #[test]
    fn headers_are_c_s_bytes_at_both_widths() {
        let env = encode_list(&["A=1", "B=2"]);
        let argv = encode_list(&["/bin/sh", "-c", "echo x"]);
        let h = Header {
            msg_type: MSG_REQUEST,
            magic: std::array::from_fn(|i| i as u8),
            request_id: 7,
            env_size: env.len(),
            argv_size: argv.len(),
            data_size: 0,
            instance_type: TYPE_EXEC,
        };
        let magic = "00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f";
        let wide = hex(&format!(
            "01 {magic} 07 00 00 00 00 00 00 00 09 00 00 00 00 00 00 00 13 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00"
        ));
        let narrow = hex(&format!("01 {magic} 07 00 00 00 09 00 00 00 13 00 00 00 00 00 00 00 00"));
        assert_eq!((h.encode_width(8), h.encode_width(4)), (wide.clone(), narrow.clone()));
        assert_eq!(
            (Header::decode_width(&wide, 8), Header::decode_width(&narrow, 4)),
            (Some(h), Some(h))
        );
        let body = hex("41 3d 31 00 42 3d 32 00 00 2f 62 69 6e 2f 73 68 00 2d 63 00 65 63 68 6f 20 78 00 00");
        assert_eq!([env, argv].concat(), body);
        assert_eq!(
            (Header::ping().encode_width(8), Header::ping().encode_width(4)),
            ([vec![2], vec![0; 49]].concat(), [vec![2], vec![0; 33]].concat())
        );
        assert_eq!(HEADER_LEN, Header::ping().encode().len());
        assert_eq!(Header::decode(&[0; 3]), None);
    }

    #[test]
    fn reports_are_c_s_bytes() {
        let cases = [
            (Report::started(4321), "ee 55 da ba 01 00 00 00 e1 10 00 00"),
            (Report::failed(2), "ee 55 da ba 02 00 00 00 02 00 00 00"),
            (Report::exited(0x300), "ee 55 da ba 03 00 00 00 00 03 00 00"),
            (Report::ping(), "ee 55 da ba 04 00 00 00 00 00 00 00"),
            (Report::handshake(4321), "00 00 00 00 01 00 00 00 e1 10 00 00"),
        ];
        for (report, bytes) in cases {
            let bytes: [u8; 12] = hex(bytes).try_into().unwrap();
            assert_eq!((report.encode(), Report::decode(&bytes)), (bytes, report), "{report:?}");
        }
    }

    /// `argv_encode()` and `argv_decode()` as C's own code answered (empties dropped; exactly one final empty string).
    #[test]
    fn lists_are_c_s() {
        let none: [&str; 0] = [];
        assert_eq!(
            [encode_list(&none), encode_list(&["a"]), encode_list(&["a", "", "b"]), encode_list(&[""])],
            [b"\0".to_vec(), b"a\0\0".to_vec(), b"a\0b\0\0".to_vec(), b"\0".to_vec()]
        );
        let decoded: Vec<Option<Vec<&[u8]>>> =
            [&b"\0"[..], b"a\0\0", b"a\0b\0\0", b"a\0", b"a\0\0\0", b"\0\0", b"a", b"a\0\0b\0\0", b""]
                .into_iter()
                .map(decode_list)
                .collect();
        assert_eq!(
            decoded,
            [
                Some(vec![]),
                Some(vec![&b"a"[..]]),
                Some(vec![&b"a"[..], &b"b"[..]]),
                None,
                None,
                None,
                None,
                None,
                None
            ]
        );
    }

    #[test]
    fn command_lines_as_c_prints_them() {
        assert_eq!(
            [
                cmdline(&["/bin/sh", "-c", "/x/system-info.sh"]),
                cmdline(&["/bin/sh", "-c", "a \"b\""]),
                cmdline(&["a b"]),
                cmdline(&["a", "", "b"]),
            ],
            [
                "/bin/sh -c /x/system-info.sh".to_string(),
                r#"/bin/sh -c "a \"b\"""#.to_string(),
                "a b\"".to_string(),
                "a  b".to_string()
            ]
        );
    }

    /// The raw status round trip through libc's macros, and the exit code a caller gets.
    #[test]
    fn wait_statuses_read_as_c_s() {
        use nix::sys::signal::Signal;
        use nix::sys::wait::WaitStatus;
        use nix::unistd::Pid;
        let p = Pid::from_raw(1);
        let raws = [
            raw_wait_status(WaitStatus::Exited(p, 3)),
            raw_wait_status(WaitStatus::Signaled(p, Signal::SIGTERM, false)),
            raw_wait_status(WaitStatus::Signaled(p, Signal::SIGPIPE, false)),
            raw_wait_status(WaitStatus::Signaled(p, Signal::SIGKILL, true)),
        ];
        assert_eq!(raws, [0x300, 15, 13, 9 | 0x80]);
        assert!(libc::WIFSIGNALED(raws[3]) && libc::WCOREDUMP(raws[3]) && libc::WTERMSIG(raws[3]) == 9);
        assert_eq!(raws.map(status_rc), [3, 0, 0, -1]);
    }

    #[test]
    fn the_control_message_is_c_s_length() {
        assert_eq!(cmsg_len(TRANSFER_FDS), if size_of::<usize>() == 8 { 32 } else { 28 });
    }
}
