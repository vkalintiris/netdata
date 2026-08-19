//! Host name discovery, only needed when the daemon did not pass one (manual runs).

/// The system host name, or an empty string if it cannot be determined.
pub fn full() -> String {
    #[cfg(unix)]
    {
        // SAFETY: the buffer is large enough for any host name POSIX allows, and it
        // is NUL-terminated defensively before being read back.
        unsafe {
            let mut buf = vec![0u8; 256];
            let rc = libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len() - 1);
            if rc != 0 {
                return String::new();
            }
            let end = buf.iter().position(|&b| b == 0).unwrap_or(0);
            buf.truncate(end);
            String::from_utf8_lossy(&buf).into_owned()
        }
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").unwrap_or_default()
    }
}

/// `hostname -s`: everything before the first dot.
pub fn short() -> String {
    let full = full();
    full.split('.').next().unwrap_or(&full).to_string()
}
