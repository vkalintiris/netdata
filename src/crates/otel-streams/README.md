# otel-streams

Streams real-world log events to otel-plugin via OTLP gRPC on `:4317`.

## Binaries

| Binary | Source | Type |
|---|---|---|
| `certstream` | Certificate Transparency Log (WebSocket) | Live stream |
| `jetstream` | Bluesky Jetstream firehose (WebSocket) | Live stream |

### Common options

All binaries share these flags:

```
--otel-endpoint <ADDR>      OTel gRPC endpoint  [default: http://127.0.0.1:4317]
--batch-size <N>            Max events per gRPC request  [default: 100]
--flush-interval-ms <MS>    Max ms before flushing a partial batch  [default: 1000]
--tenant-id <ID>            Tenant ID sent via X-Scope-OrgID gRPC header
--log-level <LEVEL>         Tracing log level  [default: info]
```

---

## certstream

Streams Certificate Transparency Log events from a
[certstream-server-go](https://github.com/d-Rickyy-b/certstream-server-go)
WebSocket.

### Prerequisites

```bash
docker run -d --rm --name certstream-server -p 8080:8080 0rickyy0/certstream-server-go
```

### Run

```bash
cargo run --release -p otel-streams --bin certstream
```

Source-specific: `--certstream-url <URL>` [default: `ws://127.0.0.1:8080/`].

---

## jetstream

Streams Bluesky Jetstream events (posts, likes, follows, etc.) from the
public firehose.

### Run

```bash
cargo run --release -p otel-streams --bin jetstream
```

### Filtering by collection

```bash
cargo run --release -p otel-streams --bin jetstream \
    --collections app.bsky.feed.post,app.bsky.feed.like
```

Source-specific: `--jetstream-url <URL>` [default: `wss://jetstream2.us-east.bsky.network/subscribe`], `--collections <LIST>`.
