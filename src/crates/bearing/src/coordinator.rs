use std::collections::HashMap;
use std::os::unix::io::{FromRawFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bearing_proto::bearing_query_client::BearingQueryClient;
use bearing_proto::{PingRequest, QueryLogsRequest};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A request from the FFI layer into the async coordinator.
pub struct QueryRequest {
    pub query: String,
    pub reply: oneshot::Sender<String>,
}

struct Child {
    name: String,
    client: BearingQueryClient<Channel>,
}

type ChildMap = Arc<Mutex<HashMap<u64, Child>>>;

/// Runs the coordinator loop.
pub async fn run(
    mut fd_rx: mpsc::Receiver<RawFd>,
    mut request_rx: mpsc::Receiver<QueryRequest>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    eprintln!("bearing: coordinator started (gRPC mode)");

    let children: ChildMap = Arc::new(Mutex::new(HashMap::new()));

    loop {
        tokio::select! {
            Some(fd) = fd_rx.recv() => {
                let child_id = next_id();
                let children = children.clone();
                tokio::spawn(async move {
                    if let Err(e) = register_child(child_id, fd, children).await {
                        eprintln!("bearing: failed to register child {child_id}: {e}");
                    }
                });
            }

            Some(req) = request_rx.recv() => {
                let children = children.clone();
                tokio::spawn(handle_query(req, children));
            }

            _ = &mut shutdown_rx => {
                eprintln!("bearing: shutting down");
                break;
            }
        }
    }
}

/// Take a raw fd, send the welcome line, create a tonic gRPC client,
/// ping the child to learn its name, and register it.
async fn register_child(
    child_id: u64,
    fd: RawFd,
    children: ChildMap,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let std_stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
    std_stream.set_nonblocking(true)?;
    let stream = TcpStream::from_std(std_stream)?;

    // Create a tonic client channel from the taken-over socket.
    // No welcome message — both sides go straight to HTTP/2.
    // The closure must be FnMut, so we use Option::take to move
    // the stream out on the first (and only) call.
    let mut stream_opt = Some(stream);
    let channel = Endpoint::try_from("http://[::]:50051")?
        .connect_with_connector(service_fn(move |_: Uri| {
            let stream = stream_opt.take().expect("connector called more than once");
            let io = hyper_util::rt::TokioIo::new(stream);
            async move { Ok::<_, std::io::Error>(io) }
        }))
        .await?;

    let mut client = BearingQueryClient::new(channel);

    // Ping to learn the child's name.
    let ping_resp = client.ping(PingRequest {}).await?;
    let name = ping_resp.into_inner().name;

    eprintln!("bearing: child {child_id} registered: {name}");

    children.lock().await.insert(child_id, Child { name, client });

    Ok(())
}

async fn handle_query(req: QueryRequest, children: ChildMap) {
    let ch = children.lock().await;
    let children_count = ch.len();

    if children_count == 0 {
        let _ = req.reply.send(r#"{"children":0,"results":[]}"#.to_string());
        return;
    }

    // Snapshot: clone clients and names so we can drop the lock before awaiting.
    let mut work: Vec<(String, BearingQueryClient<Channel>)> = Vec::new();
    for child in ch.values() {
        work.push((child.name.clone(), child.client.clone()));
    }
    drop(ch);

    // Fan out query to all children.
    let mut results = Vec::new();
    for (name, mut client) in work {
        let query_id = next_id();
        let request = QueryLogsRequest {
            id: query_id,
            query: req.query.clone(),
        };

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let response = client.query_logs(request).await?;
            let mut stream = response.into_inner();
            let mut data_parts = Vec::new();
            while let Some(msg) = stream.message().await? {
                data_parts.push(msg.data);
            }
            Ok::<_, tonic::Status>(data_parts)
        })
        .await;

        match result {
            Ok(Ok(data_parts)) => {
                let data = if data_parts.len() == 1 {
                    data_parts.into_iter().next().unwrap()
                } else {
                    format!("[{}]", data_parts.join(","))
                };
                results.push(format!(r#"{{"child":"{name}","data":{data}}}"#));
            }
            _ => {
                results.push(format!(r#"{{"child":"{name}","data":null}}"#));
            }
        }
    }

    let results_json = results.join(",");
    let resp = format!(r#"{{"children":{children_count},"results":[{results_json}]}}"#);
    let _ = req.reply.send(resp);
}
