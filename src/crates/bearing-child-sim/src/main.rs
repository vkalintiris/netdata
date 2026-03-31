use std::pin::Pin;

use bearing_proto::bearing_query_server::{BearingQuery, BearingQueryServer};
use bearing_proto::{PingRequest, PingResponse, QueryLogsRequest, QueryLogsResponse};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};

const DEFAULT_ADDR: &str = "127.0.0.1:19999";

struct MyBearingQuery {
    name: String,
}

#[tonic::async_trait]
impl BearingQuery for MyBearingQuery {
    async fn ping(&self, _req: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        Ok(Response::new(PingResponse {
            name: self.name.clone(),
        }))
    }

    type QueryLogsStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<QueryLogsResponse, Status>> + Send>>;

    async fn query_logs(
        &self,
        req: Request<QueryLogsRequest>,
    ) -> Result<Response<Self::QueryLogsStream>, Status> {
        let inner = req.into_inner();
        eprintln!("received query {}: {}", inner.id, inner.query);

        let response = QueryLogsResponse {
            query_id: inner.id,
            is_partial: false,
            data: format!(r#"{{"echo":"{}"}}"#, inner.query),
        };

        let stream = tokio_stream::once(Ok(response));
        Ok(Response::new(Box::pin(stream)))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or(DEFAULT_ADDR.into());
    let child_name = std::env::args().nth(2).unwrap_or("child-sim-1".into());

    eprintln!("connecting to {addr} as {child_name}...");

    let mut stream = TcpStream::connect(&addr)
        .await
        .expect("failed to connect");

    // Send BEARING handshake to trigger connection upgrade.
    stream
        .write_all(b"BEARING / HTTP/1.1\r\n\r\n")
        .await
        .expect("failed to send BEARING handshake");

    eprintln!("starting gRPC server...");

    // Run tonic server directly on the taken-over socket.
    // Chain with pending() so the server loop stays alive after accepting
    // the one connection (otherwise serve_with_incoming returns immediately).
    let incoming = tokio_stream::once(Ok::<_, std::io::Error>(stream))
        .chain(tokio_stream::pending());

    tonic::transport::Server::builder()
        .add_service(BearingQueryServer::new(MyBearingQuery { name: child_name }))
        .serve_with_incoming(incoming)
        .await
        .expect("gRPC server failed");

    eprintln!("gRPC server exited");
}
