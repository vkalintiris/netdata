//! A gRPC tee in front of the lab agent's OTLP listener, for the calculator's
//! live comparisons.
//!
//! The collector exports to the tee; the tee forwards every request's bytes
//! unchanged to the agent and answers with the agent's answer. Traces and logs
//! exports are also written to a capture file (`otel_oracle::capture`) with
//! their receive time, the agent's gRPC status and its rejected count. While the
//! freeze file exists, traces and logs exports are refused with `UNAVAILABLE`
//! and neither forwarded nor recorded, so the store and the capture stay still
//! while the agent is queried.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bytes::{Buf, BufMut, Bytes};
use clap::Parser;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceResponse;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceResponse;
use prost::Message;
use tonic::codec::{
    Codec, CompressionEncoding, DecodeBuf, Decoder, EnabledCompressionEncodings, EncodeBuf, Encoder,
};
use tonic::codegen::{BoxFuture, Context, Poll, Service, StdError, http};
use tonic::transport::Channel;
use tonic::{Status, body::Body};

use otel_oracle::capture::{self, Record, Signal};

/// Largest request the tee accepts or forwards.
const MAX_MESSAGE_BYTES: usize = 256 << 20;

#[derive(Parser)]
#[command(name = "otel-tee")]
#[command(about = "Record OTLP traces and logs exports on their way to a Netdata agent")]
struct Args {
    /// Address the collector exports to.
    #[arg(long, default_value = "0.0.0.0:34317")]
    listen: SocketAddr,
    /// The agent's OTLP/gRPC listener, e.g. http://127.0.0.1:34318.
    #[arg(long)]
    upstream: String,
    /// Directory for the capture file (one per run).
    #[arg(long)]
    out: PathBuf,
    /// While this file exists, traces and logs exports are refused.
    #[arg(long)]
    freeze_file: PathBuf,
}

/// Messages pass through as bytes: the tee never decodes a request.
#[derive(Clone, Copy, Default)]
struct RawCodec;

impl Codec for RawCodec {
    type Encode = Bytes;
    type Decode = Bytes;
    type Encoder = RawCodec;
    type Decoder = RawCodec;

    fn encoder(&mut self) -> Self::Encoder {
        RawCodec
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawCodec
    }
}

impl Encoder for RawCodec {
    type Item = Bytes;
    type Error = Status;

    fn encode(&mut self, item: Bytes, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
        dst.put(item);
        Ok(())
    }
}

impl Decoder for RawCodec {
    type Item = Bytes;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Bytes>, Status> {
        let len = src.remaining();
        Ok(Some(src.copy_to_bytes(len)))
    }
}

struct Tee {
    upstream: Channel,
    capture: Mutex<std::io::BufWriter<std::fs::File>>,
    freeze_file: PathBuf,
}

impl Tee {
    async fn relay(
        &self,
        path: &'static str,
        signal: Option<Signal>,
        request: tonic::Request<Bytes>,
    ) -> Result<tonic::Response<Bytes>, Status> {
        if signal.is_some() && self.freeze_file.exists() {
            return Err(Status::unavailable("otel-tee is frozen"));
        }
        let received_unix_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
        let (metadata, _, message) = request.into_parts();
        let forwarded =
            tonic::Request::from_parts(metadata, tonic::Extensions::default(), message.clone());

        let mut client = tonic::client::Grpc::new(self.upstream.clone())
            .send_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Gzip)
            .max_decoding_message_size(MAX_MESSAGE_BYTES)
            .max_encoding_message_size(MAX_MESSAGE_BYTES);
        client
            .ready()
            .await
            .map_err(|e| Status::unavailable(format!("agent not ready: {e}")))?;
        let answer = client
            .unary(
                forwarded,
                http::uri::PathAndQuery::from_static(path),
                RawCodec,
            )
            .await;

        if let Some(signal) = signal {
            let (grpc_code, rejected) = match &answer {
                Ok(response) => (0, rejected(signal, response.get_ref())),
                Err(status) => (status.code() as i32, 0),
            };
            let record = Record {
                received_unix_ns,
                signal,
                grpc_code,
                rejected,
                request: message.to_vec(),
            };
            let mut capture = self
                .capture
                .lock()
                .map_err(|_| Status::internal("capture lock poisoned"))?;
            let written = capture::write_record(&mut *capture, &record)
                .and_then(|()| std::io::Write::flush(&mut *capture));
            if let Err(e) = written {
                // A capture with a hole would be compared as if complete: stop.
                eprintln!("otel-tee: writing the capture failed: {e}");
                std::process::exit(1);
            }
        }
        answer
    }
}

/// Items the agent reported rejecting; an answer that does not decode counts
/// as rejecting nothing it can name, so it is flagged as one rejection.
fn rejected(signal: Signal, answer: &Bytes) -> i64 {
    let partial = match signal {
        Signal::Traces => ExportTraceServiceResponse::decode(answer.as_ref())
            .map(|a| a.partial_success.map_or(0, |p| p.rejected_spans)),
        Signal::Logs => ExportLogsServiceResponse::decode(answer.as_ref())
            .map(|a| a.partial_success.map_or(0, |p| p.rejected_log_records)),
    };
    partial.unwrap_or(1)
}

/// One OTLP service: its `Export` method is relayed, anything else is
/// unimplemented.
#[derive(Clone)]
struct ExportService {
    tee: Arc<Tee>,
    path: &'static str,
    signal: Option<Signal>,
}

struct ExportCall(ExportService);

impl tonic::server::UnaryService<Bytes> for ExportCall {
    type Response = Bytes;
    type Future = BoxFuture<tonic::Response<Bytes>, Status>;

    fn call(&mut self, request: tonic::Request<Bytes>) -> Self::Future {
        let service = self.0.clone();
        Box::pin(async move {
            service
                .tee
                .relay(service.path, service.signal, request)
                .await
        })
    }
}

impl<B> Service<http::Request<B>> for ExportService
where
    B: tonic::codegen::Body + Send + 'static,
    B::Error: Into<StdError> + Send + 'static,
{
    type Response = http::Response<Body>;
    type Error = std::convert::Infallible;
    type Future = BoxFuture<Self::Response, Self::Error>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        if request.uri().path() != self.path {
            let path = request.uri().path().to_string();
            return Box::pin(async move {
                Ok(Status::unimplemented(format!("otel-tee does not relay {path}")).into_http())
            });
        }
        let call = ExportCall(self.clone());
        Box::pin(async move {
            let mut compression = EnabledCompressionEncodings::default();
            compression.enable(CompressionEncoding::Gzip);
            let mut grpc = tonic::server::Grpc::new(RawCodec)
                .apply_compression_config(compression, compression)
                .apply_max_message_size_config(Some(MAX_MESSAGE_BYTES), Some(MAX_MESSAGE_BYTES));
            Ok(grpc.unary(call, request).await)
        })
    }
}

macro_rules! named_service {
    ($name:ident, $service:literal) => {
        #[derive(Clone)]
        struct $name(ExportService);

        impl $name {
            fn new(tee: &Arc<Tee>, signal: Option<Signal>) -> Self {
                $name(ExportService {
                    tee: Arc::clone(tee),
                    path: concat!("/", $service, "/Export"),
                    signal,
                })
            }
        }

        impl tonic::server::NamedService for $name {
            const NAME: &'static str = $service;
        }

        impl<B> Service<http::Request<B>> for $name
        where
            B: tonic::codegen::Body + Send + 'static,
            B::Error: Into<StdError> + Send + 'static,
        {
            type Response = http::Response<Body>;
            type Error = std::convert::Infallible;
            type Future = BoxFuture<Self::Response, Self::Error>;

            fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                Service::<http::Request<B>>::poll_ready(&mut self.0, cx)
            }

            fn call(&mut self, request: http::Request<B>) -> Self::Future {
                self.0.call(request)
            }
        }
    };
}

named_service!(
    TracesTee,
    "opentelemetry.proto.collector.trace.v1.TraceService"
);
named_service!(LogsTee, "opentelemetry.proto.collector.logs.v1.LogsService");
named_service!(
    MetricsTee,
    "opentelemetry.proto.collector.metrics.v1.MetricsService"
);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    std::fs::create_dir_all(&args.out)?;
    let path = args.out.join(format!("capture-{}.otee", started.as_secs()));
    let mut file = std::io::BufWriter::new(std::fs::File::create_new(&path)?);
    capture::write_header(&mut file)?;
    std::io::Write::flush(&mut file)?;

    let tee = Arc::new(Tee {
        upstream: Channel::from_shared(args.upstream.clone())?.connect_lazy(),
        capture: Mutex::new(file),
        freeze_file: args.freeze_file,
    });
    eprintln!(
        "otel-tee: {} -> {}, capture {}",
        args.listen,
        args.upstream,
        path.display()
    );
    tonic::transport::Server::builder()
        .add_service(TracesTee::new(&tee, Some(Signal::Traces)))
        .add_service(LogsTee::new(&tee, Some(Signal::Logs)))
        .add_service(MetricsTee::new(&tee, None))
        .serve(args.listen)
        .await?;
    Ok(())
}
