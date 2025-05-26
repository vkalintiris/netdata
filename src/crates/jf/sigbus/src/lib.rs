use error::{JournalError, Result};
use std::sync::atomic::{AtomicBool, Ordering};

static SIGBUS_OCCURRED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigbus_handler(
    _sig: libc::c_int,
    info: *mut libc::siginfo_t,
    _ucontext: *mut libc::c_void,
) {
    unsafe {
        let si = &*info;
        let fault_addr = si.si_addr();

        let page_addr = (fault_addr as usize & !(4096 - 1)) as *mut libc::c_void;
        libc::mmap(
            page_addr,
            4096,
            libc::PROT_READ,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED,
            -1,
            0,
        );

        SIGBUS_OCCURRED.store(true, Ordering::Relaxed);
    }
}

pub fn sigbus_occurred() -> bool {
    SIGBUS_OCCURRED.load(Ordering::Relaxed)
}

pub fn sigbus_install_handler() -> Result<()> {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();

        sa.sa_flags = libc::SA_SIGINFO;
        sa.sa_sigaction = sigbus_handler as usize;

        if libc::sigaction(libc::SIGBUS, &sa, std::ptr::null_mut()) == -1 {
            return Err(JournalError::SigbusHandlerError);
        }
    }

    Ok(())
}
