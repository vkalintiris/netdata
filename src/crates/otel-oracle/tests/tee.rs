//! `otel-tee` between a client and a stand-in agent: exports are forwarded with
//! the agent's answer, traces and logs are recorded with their outcome, and a
//! frozen tee refuses exports without forwarding or recording them.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_client::LogsServiceClient;
use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::{
    LogsService, LogsServiceServer,
};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::{
    TraceService, TraceServiceServer,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use otel_oracle::capture::{self, Signal};
use tonic::codec::CompressionEncoding;

/// A stand-in agent: rejects two spans of every traces export, accepts logs.
#[derive(Clone, Default)]
struct Agent {
    exports: Arc<AtomicUsize>,
}

#[tonic::async_trait]
impl TraceService for Agent {
    async fn export(
        &self,
        _request: tonic::Request<ExportTraceServiceRequest>,
    ) -> Result<tonic::Response<ExportTraceServiceResponse>, tonic::Status> {
        self.exports.fetch_add(1, Ordering::SeqCst);
        Ok(tonic::Response::new(ExportTraceServiceResponse {
            partial_success: Some(ExportTracePartialSuccess {
                rejected_spans: 2,
                error_message: "too old".to_string(),
            }),
        }))
    }
}

#[tonic::async_trait]
impl LogsService for Agent {
    async fn export(
        &self,
        _request: tonic::Request<ExportLogsServiceRequest>,
    ) -> Result<tonic::Response<ExportLogsServiceResponse>, tonic::Status> {
        self.exports.fetch_add(1, Ordering::SeqCst);
        Ok(tonic::Response::new(ExportLogsServiceResponse::default()))
    }
}

fn traces_request() -> ExportTraceServiceRequest {
    let span = Span {
        trace_id: vec![7; 16],
        span_id: vec![1; 8],
        name: "GET /".to_string(),
        start_time_unix_nano: 1_700_000_000_000_000_000,
        end_time_unix_nano: 1_700_000_000_500_000_000,
        ..Default::default()
    };
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![span],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

#[tokio::test]
async fn the_tee_forwards_records_and_freezes() {
    let agent = Agent::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(
                TraceServiceServer::new(agent.clone()).accept_compressed(CompressionEncoding::Gzip),
            )
            .add_service(
                LogsServiceServer::new(agent.clone()).accept_compressed(CompressionEncoding::Gzip),
            )
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );

    let dir = tempfile::tempdir().unwrap();
    let freeze = dir.path().join("freeze");
    let listen = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut tee = std::process::Command::new(env!("CARGO_BIN_EXE_otel-tee"))
        .arg("--listen")
        .arg(listen.to_string())
        .arg("--upstream")
        .arg(format!("http://{upstream}"))
        .arg("--out")
        .arg(dir.path())
        .arg("--freeze-file")
        .arg(&freeze)
        .spawn()
        .unwrap();

    let endpoint = format!("http://{listen}");
    let mut traces = None;
    for _ in 0..100 {
        if let Ok(client) = TraceServiceClient::connect(endpoint.clone()).await {
            traces = Some(client.send_compressed(CompressionEncoding::Gzip));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut traces = traces.expect("the tee listens");
    let mut logs = LogsServiceClient::connect(endpoint).await.unwrap();

    let answer = traces.export(traces_request()).await.unwrap().into_inner();
    assert_eq!(answer.partial_success.map(|p| p.rejected_spans), Some(2));
    logs.export(ExportLogsServiceRequest::default())
        .await
        .unwrap();
    assert_eq!(agent.exports.load(Ordering::SeqCst), 2);

    std::fs::write(&freeze, b"").unwrap();
    let refused = traces.export(traces_request()).await.unwrap_err();
    assert_eq!(refused.code(), tonic::Code::Unavailable);
    assert_eq!(
        agent.exports.load(Ordering::SeqCst),
        2,
        "a frozen tee forwards nothing"
    );

    tee.kill().unwrap();
    tee.wait().unwrap();
    let mut files = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "otee"));
    let path = files.next().expect("one capture file");
    let records = capture::read_all(&mut std::fs::File::open(path).unwrap()).unwrap();
    assert_eq!(records.len(), 2, "the refused export is not recorded");
    assert_eq!(
        (records[0].signal, records[0].grpc_code, records[0].rejected),
        (Signal::Traces, 0, 2)
    );
    assert_eq!(records[0].traces().unwrap(), traces_request());
    assert!(!records[0].accepted());
    assert_eq!(records[1].signal, Signal::Logs);
    assert!(records[1].accepted());
    assert!(records[0].received_unix_ns <= records[1].received_unix_ns);
}
