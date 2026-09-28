//! `SIGNAL_CODE` (`src/libnetdata/signals/signal-code.c`): a signal number and its `si_code` in one 64-bit value,
//! printed as `SIGSEGV/SEGV_MAPERR` and parsed back, with Linux's (glibc's) values (D88.8).

use netdata_agent_text::parse::str2i;

/// `SIGNAL_CODE_CREATE()`.
pub const fn create(signo: i32, si_code: i32) -> u64 {
    ((signo as u64) << 32) | (si_code as u32 as u64)
}

/// `SIGNAL_NUM_names[]`.
const SIGNALS: [(i32, &str); 31] = [
    (1, "SIGHUP"),
    (2, "SIGINT"),
    (3, "SIGQUIT"),
    (4, "SIGILL"),
    (5, "SIGTRAP"),
    (6, "SIGABRT"),
    (7, "SIGBUS"),
    (8, "SIGFPE"),
    (9, "SIGKILL"),
    (10, "SIGUSR1"),
    (11, "SIGSEGV"),
    (12, "SIGUSR2"),
    (13, "SIGPIPE"),
    (14, "SIGALRM"),
    (15, "SIGTERM"),
    (16, "SIGSTKFLT"),
    (17, "SIGCHLD"),
    (18, "SIGCONT"),
    (19, "SIGSTOP"),
    (20, "SIGTSTP"),
    (21, "SIGTTIN"),
    (22, "SIGTTOU"),
    (23, "SIGURG"),
    (24, "SIGXCPU"),
    (25, "SIGXFSZ"),
    (26, "SIGVTALRM"),
    (27, "SIGPROF"),
    (28, "SIGWINCH"),
    (29, "SIGPOLL"),
    (30, "SIGPWR"),
    (31, "SIGSYS"),
];

/// `SI_CODE_names[]`.
const SI_CODES: [(i32, &str); 10] = [
    (-60, "SI_ASYNCNL"),
    (-7, "SI_DETHREAD"),
    (-6, "SI_TKILL"),
    (-5, "SI_SIGIO"),
    (-4, "SI_ASYNCIO"),
    (-3, "SI_MESGQ"),
    (-2, "SI_TIMER"),
    (-1, "SI_QUEUE"),
    (0, "SI_USER"),
    (0x80, "SI_KERNEL"),
];

const SIGILL: i32 = 4;
const SIGTRAP: i32 = 5;
const SIGBUS: i32 = 7;
const SIGFPE: i32 = 8;
const SIGSEGV: i32 = 11;

/// `SIGNAL_CODE_names[]`: the codes of each signal.
const CODES: [(u64, &str); 39] = [
    (create(SIGILL, 1), "ILL_ILLOPC"),
    (create(SIGILL, 2), "ILL_ILLOPN"),
    (create(SIGILL, 3), "ILL_ILLADR"),
    (create(SIGILL, 4), "ILL_ILLTRP"),
    (create(SIGILL, 5), "ILL_PRVOPC"),
    (create(SIGILL, 6), "ILL_PRVREG"),
    (create(SIGILL, 7), "ILL_COPROC"),
    (create(SIGILL, 8), "ILL_BADSTK"),
    (create(SIGILL, 9), "ILL_BADIADDR"),
    (create(SIGFPE, 1), "FPE_INTDIV"),
    (create(SIGFPE, 2), "FPE_INTOVF"),
    (create(SIGFPE, 3), "FPE_FLTDIV"),
    (create(SIGFPE, 4), "FPE_FLTOVF"),
    (create(SIGFPE, 5), "FPE_FLTUND"),
    (create(SIGFPE, 6), "FPE_FLTRES"),
    (create(SIGFPE, 7), "FPE_FLTINV"),
    (create(SIGFPE, 8), "FPE_FLTSUB"),
    (create(SIGFPE, 14), "FPE_FLTUNK"),
    (create(SIGFPE, 15), "FPE_CONDTRAP"),
    (create(SIGSEGV, 1), "SEGV_MAPERR"),
    (create(SIGSEGV, 2), "SEGV_ACCERR"),
    (create(SIGSEGV, 3), "SEGV_BNDERR"),
    (create(SIGSEGV, 4), "SEGV_PKUERR"),
    (create(SIGSEGV, 5), "SEGV_ACCADI"),
    (create(SIGSEGV, 6), "SEGV_ADIDERR"),
    (create(SIGSEGV, 7), "SEGV_ADIPERR"),
    (create(SIGSEGV, 8), "SEGV_MTEAERR"),
    (create(SIGSEGV, 9), "SEGV_MTESERR"),
    (create(SIGSEGV, 10), "SEGV_CPERR"),
    (create(SIGBUS, 1), "BUS_ADRALN"),
    (create(SIGBUS, 2), "BUS_ADRERR"),
    (create(SIGBUS, 3), "BUS_OBJERR"),
    (create(SIGBUS, 4), "BUS_MCEERR_AR"),
    (create(SIGBUS, 5), "BUS_MCEERR_AO"),
    (create(SIGTRAP, 1), "TRAP_BRKPT"),
    (create(SIGTRAP, 2), "TRAP_TRACE"),
    (create(SIGTRAP, 3), "TRAP_BRANCH"),
    (create(SIGTRAP, 4), "TRAP_HWBKPT"),
    (create(SIGTRAP, 5), "TRAP_UNK"),
];

fn name<T: PartialEq>(table: &[(T, &'static str)], id: T) -> Option<&'static str> {
    table.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
}

fn id<T: Copy>(table: &[(T, &str)], text: &[u8]) -> Option<T> {
    table
        .iter()
        .find(|(_, n)| n.as_bytes() == text)
        .map(|(i, _)| *i)
}

/// `SIGNAL_CODE_2str_h()` into a `UINT64_MAX_LENGTH` buffer: the signal's name (else its number), a slash, the
/// code's name (else its `si_code` name, else its number), cut at 23 bytes; empty for 0.
pub fn to_text(code: u64) -> String {
    const SIZE: usize = 24;
    if code == 0 {
        return String::new();
    }
    let signo = (code >> 32) as i32;
    let si_code = code as u32 as i32;
    let signal = name(&SIGNALS, signo).map_or_else(|| signo.to_string(), str::to_string);
    let what = name(&CODES, code)
        .or_else(|| name(&SI_CODES, si_code))
        .map_or_else(|| si_code.to_string(), str::to_string);
    let mut out = signal;
    out.truncate(SIZE - 1);
    if SIZE - 1 > out.len() {
        out.push('/');
    }
    if SIZE - 1 > out.len() {
        let room = SIZE - 1 - out.len();
        out.push_str(&what[..what.len().min(room)]);
    }
    out
}

/// `SIGNAL_CODE_2id_h()`: a signal (by name or number), then after a slash a code (a full name wins, then an
/// `si_code` name, then a number); 0 for an empty text.
pub fn from_text(text: &[u8]) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let (signal, what) = match text.iter().position(|&c| c == b'/') {
        Some(at) => (&text[..at], Some(&text[at + 1..])),
        None => (text, None),
    };
    let signo = id(&SIGNALS, signal).unwrap_or_else(|| str2i(signal));
    if let Some(code) = what.and_then(|w| id(&CODES, w)) {
        return code;
    }
    let si_code = match what {
        Some(w) => id(&SI_CODES, w).unwrap_or_else(|| str2i(w)),
        None => 0,
    };
    create(signo, si_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_codes_as_c() {
        for (code, text) in [
            (0, ""),
            (create(11, 1), "SIGSEGV/SEGV_MAPERR"),
            (create(15, 0), "SIGTERM/SI_USER"),
            (create(6, -6), "SIGABRT/SI_TKILL"),
            // a code of another signal is not a name of this one
            (create(7, 10), "SIGBUS/10"),
            (create(99, 1), "99/1"),
        ] {
            assert_eq!(to_text(code), text, "{code:#x}");
            if code != 0 {
                assert_eq!(from_text(text.as_bytes()), code, "{text}");
            }
        }
        // a code's name carries its own signal
        assert_eq!(from_text(b"SIGBUS/SEGV_MAPERR"), create(11, 1));
        assert_eq!(from_text(b"SIGSEGV"), create(11, 0));
        assert_eq!(from_text(b"11/1"), create(11, 1));
    }
}
