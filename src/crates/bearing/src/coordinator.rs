use std::collections::HashMap;
use std::os::unix::io::{FromRawFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A request from the FFI layer into the async coordinator.
pub struct QueryRequest {
    pub query: String,
    pub reply: oneshot::Sender<String>,
}

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<String>>>>;
type ChildMap = Arc<Mutex<HashMap<u64, Child>>>;

struct Child {
    name: String,
    writer: tokio::io::WriteHalf<TcpStream>,
}

/// Runs the coordinator loop. Receives new connection fds from `fd_rx`
/// and query requests from `request_rx`.
pub async fn run(
    mut fd_rx: mpsc::Receiver<RawFd>,
    mut request_rx: mpsc::Receiver<QueryRequest>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    eprintln!("bearing: coordinator started");

    let children: ChildMap = Arc::new(Mutex::new(HashMap::new()));
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

    loop {
        tokio::select! {
            // A new child fd was handed to us from the C web server.
            Some(fd) = fd_rx.recv() => {
                let stream = unsafe {
                    let std_stream = std::net::TcpStream::from_raw_fd(fd);
                    std_stream.set_nonblocking(true).ok();
                    TcpStream::from_std(std_stream)
                };
                match stream {
                    Ok(stream) => {
                        let child_id = next_id();
                        let children = children.clone();
                        let pending = pending.clone();
                        tokio::spawn(handle_child(child_id, stream, children, pending));
                    }
                    Err(e) => {
                        eprintln!("bearing: failed to wrap fd: {e}");
                    }
                }
            }

            // Process a query from FFI.
            Some(req) = request_rx.recv() => {
                let children = children.clone();
                let pending = pending.clone();
                tokio::spawn(handle_query(req, children, pending));
            }

            _ = &mut shutdown_rx => {
                eprintln!("bearing: shutting down");
                break;
            }
        }
    }
}

async fn handle_child(
    child_id: u64,
    stream: TcpStream,
    children: ChildMap,
    pending: PendingMap,
) {
    let (read_half, write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    // Send welcome.
    let mut writer = write_half;
    if writer.write_all(b"BEARING OK\n").await.is_err() {
        return;
    }

    // Read READY <name>.
    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let child_name = line
        .trim()
        .strip_prefix("READY ")
        .unwrap_or(&format!("child-{child_id}"))
        .to_string();

    eprintln!("bearing: child connected: {child_name}");

    children.lock().await.insert(
        child_id,
        Child {
            name: child_name,
            writer,
        },
    );

    // Read responses.
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("RESULT ") {
            if let Some((id_str, data)) = rest.split_once(' ') {
                if let Ok(qid) = id_str.parse::<u64>() {
                    if let Some(tx) = pending.lock().await.remove(&qid) {
                        let _ = tx.send(data.to_string());
                    }
                }
            }
        }
    }

    eprintln!("bearing: child {child_id} disconnected");
    children.lock().await.remove(&child_id);
}

async fn handle_query(req: QueryRequest, children: ChildMap, pending: PendingMap) {
    let mut ch = children.lock().await;
    let children_count = ch.len();

    if children_count == 0 {
        let resp = r#"{"children":0,"results":[]}"#.to_string();
        let _ = req.reply.send(resp);
        return;
    }

    let mut waiters = Vec::new();
    let mut names = Vec::new();
    let mut disconnected = Vec::new();

    for (&cid, child) in ch.iter_mut() {
        let qid = next_id();
        let msg = format!("QUERY {qid} {}\n", req.query);

        match child.writer.write_all(msg.as_bytes()).await {
            Ok(()) => {
                let (tx, rx) = oneshot::channel();
                pending.lock().await.insert(qid, tx);
                waiters.push(rx);
                names.push(child.name.clone());
            }
            Err(_) => {
                disconnected.push(cid);
            }
        }
    }

    for cid in disconnected {
        ch.remove(&cid);
    }
    drop(ch);

    let mut results = Vec::new();
    for (rx, name) in waiters.into_iter().zip(names) {
        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(data)) => results.push(format!(r#"{{"child":"{name}","data":{data}}}"#)),
            _ => results.push(format!(r#"{{"child":"{name}","data":null}}"#)),
        }
    }

    let results_json = results.join(",");
    let resp = format!(r#"{{"children":{children_count},"results":[{results_json}]}}"#);
    let _ = req.reply.send(resp);
}
