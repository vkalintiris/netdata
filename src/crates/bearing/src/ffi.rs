use std::os::unix::io::RawFd;
use std::ptr;
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;

use tokio::sync::{mpsc, oneshot};

use crate::coordinator;

const PONG_RESPONSE: &[u8] = b"{\"status\":\"pong\"}";

struct BearingState {
    fd_tx: mpsc::Sender<RawFd>,
    request_tx: mpsc::Sender<coordinator::QueryRequest>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

static STATE: OnceLock<Mutex<BearingState>> = OnceLock::new();

/// Initialize the bearing coordinator. Spawns a tokio runtime in a background thread.
#[unsafe(no_mangle)]
pub extern "C" fn bearing_init() {
    let (fd_tx, fd_rx) = mpsc::channel(64);
    let (request_tx, request_rx) = mpsc::channel(64);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let thread = std::thread::Builder::new()
        .name("bearing".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("bearing: failed to create tokio runtime");

            rt.block_on(coordinator::run(fd_rx, request_rx, shutdown_rx));
        })
        .expect("bearing: failed to spawn thread");

    let state = BearingState {
        fd_tx,
        request_tx,
        shutdown_tx: Some(shutdown_tx),
        thread: Some(thread),
    };

    if STATE.set(Mutex::new(state)).is_err() {
        eprintln!("bearing: already initialized");
    }
}

/// Shut down the bearing coordinator.
#[unsafe(no_mangle)]
pub extern "C" fn bearing_shutdown() {
    let Some(state_mutex) = STATE.get() else {
        return;
    };
    let mut state = state_mutex.lock().unwrap();

    if let Some(tx) = state.shutdown_tx.take() {
        let _ = tx.send(());
    }
    if let Some(handle) = state.thread.take() {
        let _ = handle.join();
    }
    eprintln!("bearing: shut down complete");
}

/// Accept a new child connection fd from the C web server.
/// The coordinator takes ownership of the fd.
#[unsafe(no_mangle)]
pub extern "C" fn bearing_accept_fd(fd: i32) -> i32 {
    let Some(state_mutex) = STATE.get() else {
        return -1;
    };
    let state = state_mutex.lock().unwrap();
    match state.fd_tx.blocking_send(fd as RawFd) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// Simple ping — no async, just returns pong.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bearing_ping(out_buf: *mut u8, out_cap: usize, out_len: *mut usize) -> i32 {
    if out_buf.is_null() || out_len.is_null() || out_cap < PONG_RESPONSE.len() {
        return -1;
    }
    unsafe {
        ptr::copy_nonoverlapping(PONG_RESPONSE.as_ptr(), out_buf, PONG_RESPONSE.len());
        *out_len = PONG_RESPONSE.len();
    }
    0
}

/// Send a query through the coordinator to all connected children.
/// Blocks until the response is ready (or timeout).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bearing_query(
    query_buf: *const u8,
    query_len: usize,
    out_buf: *mut u8,
    out_cap: usize,
    out_len: *mut usize,
) -> i32 {
    if query_buf.is_null() || out_buf.is_null() || out_len.is_null() {
        return -1;
    }

    let query = unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(query_buf, query_len))
    };
    let query = match query {
        Ok(s) => s.to_string(),
        Err(_) => return -1,
    };

    let Some(state_mutex) = STATE.get() else {
        return -2;
    };

    let tx = {
        let state = state_mutex.lock().unwrap();
        state.request_tx.clone()
    };

    let (reply_tx, reply_rx) = oneshot::channel();
    let req = coordinator::QueryRequest {
        query,
        reply: reply_tx,
    };

    if tx.blocking_send(req).is_err() {
        return -3;
    }

    let response = match reply_rx.blocking_recv() {
        Ok(r) => r,
        Err(_) => return -3,
    };

    let response_bytes = response.as_bytes();
    if response_bytes.len() > out_cap {
        return -4;
    }

    unsafe {
        ptr::copy_nonoverlapping(response_bytes.as_ptr(), out_buf, response_bytes.len());
        *out_len = response_bytes.len();
    }
    0
}
