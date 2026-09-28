//! Deadly signals (decision D91.1): a handler that runs on the thread that got the signal, with every signal masked
//! and on that thread's own stack (no `SA_ONSTACK`, as C: std's alternate stack is too small for a status file save),
//! and the end C gives them: the default action back and the signal raised again.

use std::sync::OnceLock;

use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

/// What a deadly signal's handler is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadly {
    pub signal: i32,
    pub si_code: i32,
    /// `si_addr` for SIGSEGV, SIGBUS, SIGILL and SIGFPE (whatever the kernel put there), else 0.
    pub fault_address: u64,
}

static HANDLER: OnceLock<fn(&Deadly)> = OnceLock::new();

extern "C" fn trampoline(signo: libc::c_int, info: *mut libc::siginfo_t, _context: *mut libc::c_void) {
    if info.is_null() {
        return;
    }
    // SAFETY: the kernel hands an SA_SIGINFO handler a valid `siginfo_t` for the handler's duration; `si_addr()`
    // reads the union's first member, as C's `info->si_addr` does.
    let (si_code, address) = unsafe { ((*info).si_code, (*info).si_addr() as usize as u64) };
    let faults = matches!(signo, libc::SIGSEGV | libc::SIGBUS | libc::SIGILL | libc::SIGFPE);
    let deadly = Deadly { signal: signo, si_code, fault_address: if faults { address } else { 0 } };
    if let Some(handler) = HANDLER.get() {
        handler(&deadly);
    }
}

/// `nd_initialize_signals()`'s deadly part: `handler` for each of `signals`, with every signal masked while it runs.
/// The handler must not allocate or block (D91.2).
pub fn install_deadly(signals: &[Signal], handler: fn(&Deadly)) -> nix::Result<()> {
    let _ = HANDLER.set(handler);
    let action = SigAction::new(SigHandler::SigAction(trampoline), SaFlags::SA_SIGINFO, SigSet::all());
    for &signal in signals {
        // SAFETY: the trampoline reads only its `siginfo_t` and calls a handler that allocates nothing and takes no
        // lock it could wait on (D91.2).
        unsafe { sigaction(signal, &action) }?;
    }
    Ok(())
}

/// The end of a deadly signal: the default action back, then the signal again, which the process dies by once the
/// handler returns (it stays masked until then).
pub fn die_by(signal: Signal) {
    let action = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
    // SAFETY: the default action has no handler.
    let _ = unsafe { sigaction(signal, &action) };
    let _ = nix::sys::signal::raise(signal);
}
