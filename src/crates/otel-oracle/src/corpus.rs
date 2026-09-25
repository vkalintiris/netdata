//! Deterministic multi-service trace corpora.
//!
//! The explorer's numbers depend on shapes a single-service generator cannot
//! produce: requests that cross services, errors that start deep in a request
//! and propagate up (or are handled on the way), parallel and asynchronous
//! children, orphans, extra roots, resends and zero-length spans. Every value
//! is a pure function of [`MeshParams`], so a failing comparison reproduces
//! from its seed.
//!
//! Spans come back in arrival order (by end time, as SDKs export a span when
//! it ends), so a test can cut the list into WAL files the way the plugin
//! would receive it, including traces whose spans land in different files.

use opentelemetry_proto::tonic::{
    collector::trace::v1::ExportTraceServiceRequest,
    common::v1::{AnyValue, ArrayValue, InstrumentationScope, KeyValue, any_value},
    resource::v1::Resource,
    trace::v1::{
        ResourceSpans, ScopeSpans, Span, Status,
        span::{Event, Link, SpanKind},
        status::StatusCode,
    },
};

/// A service of the simulated application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Service {
    LoadGenerator,
    Frontend,
    Cart,
    Checkout,
    Currency,
    Payment,
    Accounting,
    ProductCatalog,
    Flagd,
}

impl Service {
    pub const ALL: [Service; 9] = [
        Service::LoadGenerator,
        Service::Frontend,
        Service::Cart,
        Service::Checkout,
        Service::Currency,
        Service::Payment,
        Service::Accounting,
        Service::ProductCatalog,
        Service::Flagd,
    ];

    /// The `service.name` resource attribute.
    pub fn name(self) -> &'static str {
        match self {
            Service::LoadGenerator => "load-generator",
            Service::Frontend => "frontend",
            Service::Cart => "cart",
            Service::Checkout => "checkout",
            Service::Currency => "currency",
            Service::Payment => "payment",
            Service::Accounting => "accounting",
            Service::ProductCatalog => "product-catalog",
            Service::Flagd => "flagd",
        }
    }

    /// The `host.name` resource attribute; two services share a host so the
    /// node pivot has a many-to-one case.
    pub fn host(self) -> &'static str {
        match self {
            Service::LoadGenerator => "oracle-host-lg",
            Service::Frontend => "oracle-host-web",
            Service::Cart | Service::Checkout => "oracle-host-app",
            Service::Currency => "oracle-host-fx",
            Service::Payment => "oracle-host-pay",
            Service::Accounting => "oracle-host-acct",
            Service::ProductCatalog => "oracle-host-catalog",
            Service::Flagd => "oracle-host-flags",
        }
    }
}

/// `service.namespace` of every generated resource.
pub const SERVICE_NAMESPACE: &str = "oracle-demo";

/// Parent span id of the orphan shape: it exists in no trace, like a caller
/// that never exported its span.
pub const ORPHAN_PARENT_SPAN_ID: [u8; 8] = [0xfe; 8];

#[derive(Debug, Clone)]
pub struct MeshParams {
    /// Number of traces.
    pub traces: usize,
    /// Start of the first trace (unix nanos).
    pub start_ns: u64,
    /// Nanoseconds between the starts of consecutive traces.
    pub trace_spacing_ns: u64,
    /// Mixed into every derived value; distinct seeds give disjoint trace ids.
    pub seed: u64,
}

/// One generated span and the service that emits it.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshSpan {
    pub service: Service,
    pub span: Span,
}

/// Which request a trace simulates, by `(seed + trace index) % 10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Browse,
    Cart,
    Checkout,
    FlagStream,
}

/// How errors are placed, by `((seed + trace index) / 10) % 5`, so every shape
/// meets every variant within 50 consecutive traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorVariant {
    None,
    /// The deepest span fails and every ancestor reports ERROR too.
    Deep,
    /// A mid-level span fails and its caller handles it: the ancestors above
    /// the caller stay unset.
    Handled,
    /// No error; entry spans report OK explicitly.
    ExplicitOk,
    /// A leaf fails alone; its ancestors stay unset.
    Leaf,
}

/// Generate the corpus in arrival order.
pub fn generate(p: &MeshParams) -> Vec<MeshSpan> {
    let mut arrivals: Vec<(u64, u64, MeshSpan)> = Vec::new();
    let mut seq = 0u64;
    for t in 0..p.traces as u64 {
        let t0 = p
            .start_ns
            .saturating_add(t.saturating_mul(p.trace_spacing_ns));
        let mut b = TraceBuilder::new(p.seed, t);
        build_trace(&mut b, t0);
        apply_edge_cases(&mut b);
        for (arrival, span) in b.finish() {
            arrivals.push((arrival, seq, span));
            seq += 1;
        }
    }
    arrivals.sort_by_key(|(arrival, seq, _)| (*arrival, *seq));
    let mut out = Vec::with_capacity(arrivals.len());
    for (_, _, span) in arrivals {
        out.push(span);
    }
    out
}

/// Group spans into export requests of at most `batch` spans each, in order;
/// inside a request, one resource per service in first-seen order.
pub fn build_requests(spans: &[MeshSpan], batch: usize) -> Vec<ExportTraceServiceRequest> {
    let mut requests = Vec::new();
    for chunk in spans.chunks(batch.max(1)) {
        let mut groups: Vec<(Service, Vec<Span>)> = Vec::new();
        for s in chunk {
            match groups.iter_mut().find(|(service, _)| *service == s.service) {
                Some((_, spans)) => spans.push(s.span.clone()),
                None => groups.push((s.service, vec![s.span.clone()])),
            }
        }
        let mut resource_spans = Vec::with_capacity(groups.len());
        for (service, spans) in groups {
            resource_spans.push(ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![
                        kv("service.name", str_val(service.name())),
                        kv("service.namespace", str_val(SERVICE_NAMESPACE)),
                        kv("service.version", str_val("1.0.0")),
                        kv("host.name", str_val(service.host())),
                    ],
                    dropped_attributes_count: 0,
                    entity_refs: vec![],
                }),
                scope_spans: vec![ScopeSpans {
                    scope: Some(InstrumentationScope {
                        name: "otel-oracle".to_string(),
                        version: "1".to_string(),
                        attributes: vec![],
                        dropped_attributes_count: 0,
                    }),
                    spans,
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            });
        }
        requests.push(ExportTraceServiceRequest { resource_spans });
    }
    requests
}

/// SplitMix64 finalizer: a stateless, well-mixed hash of one word.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn trace_id(seed: u64, t: u64) -> Vec<u8> {
    (((seed as u128) << 64) | (t as u128) | (1u128 << 127))
        .to_be_bytes()
        .to_vec()
}

/// Unique per (seed, trace, position) as long as the trace index fits in
/// 32 bits and a trace has fewer than 65,536 spans.
fn span_id(seed: u64, t: u64, j: u64) -> Vec<u8> {
    (((t & 0xffff_ffff) << 16 | j) ^ (mix(seed) << 48) | (1u64 << 63))
        .to_be_bytes()
        .to_vec()
}

struct TraceBuilder {
    seed: u64,
    t: u64,
    trace_id: Vec<u8>,
    spans: Vec<MeshSpan>,
    /// Extra delay before a span arrives, on top of its end time (resends).
    resends: Vec<(usize, u64)>,
}

impl TraceBuilder {
    fn new(seed: u64, t: u64) -> Self {
        Self {
            seed,
            t,
            trace_id: trace_id(seed, t),
            spans: Vec::new(),
            resends: Vec::new(),
        }
    }

    fn key(&self) -> u64 {
        self.seed.wrapping_add(self.t)
    }

    /// A value in `lo..=hi` derived from this trace and `salt`.
    fn pick(&self, salt: u64, lo: u64, hi: u64) -> u64 {
        lo + mix(mix(self.seed ^ salt.rotate_left(17)) ^ self.t) % (hi - lo + 1)
    }

    /// The trace's time unit: most requests are milliseconds, some
    /// sub-millisecond, a few seconds, so every duration band is populated.
    fn unit(&self) -> u64 {
        match self.pick(0x5ca1e, 0, 15) {
            0..=1 => 50_000,
            2..=5 => 500_000,
            6..=10 => 2_000_000,
            11..=13 => 20_000_000,
            14 => 200_000_000,
            _ => 2_000_000_000,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        service: Service,
        parent: Option<usize>,
        kind: SpanKind,
        name: &str,
        start: u64,
        duration: u64,
        attributes: Vec<KeyValue>,
    ) -> usize {
        let j = self.spans.len() as u64;
        let parent_span_id = match parent {
            Some(idx) => self.spans[idx].span.span_id.clone(),
            None => Vec::new(),
        };
        self.spans.push(MeshSpan {
            service,
            span: Span {
                trace_id: self.trace_id.clone(),
                span_id: span_id(self.seed, self.t, j),
                parent_span_id,
                name: name.to_string(),
                kind: kind as i32,
                start_time_unix_nano: start,
                end_time_unix_nano: start.saturating_add(duration),
                attributes,
                ..Default::default()
            },
        });
        self.spans.len() - 1
    }

    fn set_status(&mut self, idx: usize, code: StatusCode, message: &str) {
        self.spans[idx].span.status = Some(Status {
            code: code as i32,
            message: message.to_string(),
        });
    }

    /// Mark `idx` as where an error started: ERROR status plus an exception
    /// event, as instrumentation libraries record it.
    fn fail_origin(&mut self, idx: usize, exception_type: &str) {
        let message = format!("{exception_type} in trace {}", self.t);
        self.set_status(idx, StatusCode::Error, &message);
        let span = &mut self.spans[idx].span;
        let time =
            span.start_time_unix_nano + (span.end_time_unix_nano - span.start_time_unix_nano) / 2;
        span.events.push(Event {
            time_unix_nano: time,
            name: "exception".to_string(),
            attributes: vec![
                kv("exception.type", str_val(exception_type)),
                kv("exception.message", str_val(&message)),
            ],
            dropped_attributes_count: 0,
        });
    }

    fn fail_propagated(&mut self, idx: usize) {
        self.set_status(idx, StatusCode::Error, "downstream failure");
    }

    fn start(&self, idx: usize) -> u64 {
        self.spans[idx].span.start_time_unix_nano
    }

    fn end(&self, idx: usize) -> u64 {
        self.spans[idx].span.end_time_unix_nano
    }

    fn finish(self) -> Vec<(u64, MeshSpan)> {
        let mut out = Vec::with_capacity(self.spans.len() + self.resends.len());
        for s in &self.spans {
            out.push((s.span.end_time_unix_nano, s.clone()));
        }
        for (idx, delay) in &self.resends {
            let s = &self.spans[*idx];
            out.push((s.span.end_time_unix_nano.saturating_add(*delay), s.clone()));
        }
        out
    }
}

fn shape_of(key: u64) -> Shape {
    match key % 10 {
        0..=3 => Shape::Browse,
        4..=6 => Shape::Cart,
        7..=8 => Shape::Checkout,
        _ => Shape::FlagStream,
    }
}

fn variant_of(key: u64) -> ErrorVariant {
    match (key / 10) % 5 {
        0 => ErrorVariant::None,
        1 => ErrorVariant::Deep,
        2 => ErrorVariant::Handled,
        3 => ErrorVariant::ExplicitOk,
        _ => ErrorVariant::Leaf,
    }
}

fn build_trace(b: &mut TraceBuilder, t0: u64) {
    let key = b.key();
    let variant = variant_of(key);
    match shape_of(key) {
        Shape::Browse => browse(b, t0, variant),
        Shape::Cart => cart(b, t0, variant),
        Shape::Checkout => checkout(b, t0, variant),
        Shape::FlagStream => flag_stream(b, t0, variant),
    }
}

fn http_entry_attributes(b: &TraceBuilder, route: &str, failed: bool) -> Vec<KeyValue> {
    vec![
        kv(
            "http.request.method",
            str_val(route.split(' ').next().unwrap_or("GET")),
        ),
        kv(
            "http.route",
            str_val(route.split(' ').nth(1).unwrap_or(route)),
        ),
        kv(
            "http.response.status_code",
            int_val(if failed { 500 } else { 200 }),
        ),
        kv("request.id", str_val(&format!("{:016x}", mix(b.key())))),
        kv("app.synthetic", bool_val(true)),
    ]
}

/// frontend GET /api/products/{id} → product-catalog GetProduct → database.
fn browse(b: &mut TraceBuilder, t0: u64, variant: ErrorVariant) {
    let u = b.unit();
    let hop = u / 10 + 1;
    let before = b.pick(1, 1, 3) * u / 2;
    let query = b.pick(2, 1, 6) * u;
    let server = u / 4 + query + u / 2;
    let client = server + 2 * hop;
    let root_duration = before + client + u;

    let failed_entry = matches!(variant, ErrorVariant::Deep);
    let attrs = http_entry_attributes(b, "GET /api/products/{id}", failed_entry);
    let root = b.add(
        Service::Frontend,
        None,
        SpanKind::Server,
        "GET /api/products/{id}",
        t0,
        root_duration,
        attrs,
    );
    let call = b.add(
        Service::Frontend,
        Some(root),
        SpanKind::Client,
        "GetProduct",
        t0 + before,
        client,
        vec![
            kv("rpc.system", str_val("grpc")),
            kv("rpc.method", str_val("GetProduct")),
        ],
    );
    let catalog = b.add(
        Service::ProductCatalog,
        Some(call),
        SpanKind::Server,
        "GetProduct",
        b.start(call) + hop,
        server,
        vec![
            kv("rpc.system", str_val("grpc")),
            kv(
                "app.product.id",
                str_val(&format!("P{:03}", b.pick(3, 1, 40))),
            ),
        ],
    );
    let db = b.add(
        Service::ProductCatalog,
        Some(catalog),
        SpanKind::Client,
        "SELECT products",
        b.start(catalog) + u / 4,
        query,
        vec![
            kv("db.system", str_val("postgresql")),
            kv(
                "db.statement",
                str_val("SELECT * FROM products WHERE id = $1"),
            ),
            kv("app.tags", array_val(&["catalog", "read"])),
        ],
    );
    match variant {
        ErrorVariant::None => {}
        ErrorVariant::Deep => {
            b.fail_origin(db, "QueryTimeout");
            for idx in [catalog, call, root] {
                b.fail_propagated(idx);
            }
        }
        ErrorVariant::Handled => {
            b.fail_origin(catalog, "ProductNotFound");
            b.fail_propagated(call);
        }
        ErrorVariant::ExplicitOk => {
            b.set_status(root, StatusCode::Ok, "");
            b.set_status(catalog, StatusCode::Ok, "");
        }
        ErrorVariant::Leaf => b.fail_origin(db, "DeadlockDetected"),
    }
}

/// load-generator (a CLIENT root) → frontend GET /api/cart → cart GetCart →
/// two sequential cache calls with a gap between them.
fn cart(b: &mut TraceBuilder, t0: u64, variant: ErrorVariant) {
    let u = b.unit();
    let hop = u / 10 + 1;
    let first = b.pick(4, 1, 4) * u;
    let gap = u / 3;
    let second = b.pick(5, 1, 3) * u / 2;
    let server = u / 10 + first + gap + second + u / 10;
    let client = server + 2 * hop;
    let entry = u / 5 + client + u / 2;
    let root_duration = entry + 2 * hop;

    let root = b.add(
        Service::LoadGenerator,
        None,
        SpanKind::Client,
        "GET /api/cart",
        t0,
        root_duration,
        vec![
            kv("http.request.method", str_val("GET")),
            kv("url.path", str_val("/api/cart")),
        ],
    );
    let failed_entry = matches!(variant, ErrorVariant::Deep);
    let attrs = http_entry_attributes(b, "GET /api/cart", failed_entry);
    let entry_idx = b.add(
        Service::Frontend,
        Some(root),
        SpanKind::Server,
        "GET /api/cart",
        t0 + hop,
        entry,
        attrs,
    );
    let call = b.add(
        Service::Frontend,
        Some(entry_idx),
        SpanKind::Client,
        "GetCart",
        b.start(entry_idx) + u / 5,
        client,
        vec![kv("rpc.system", str_val("grpc"))],
    );
    let cart_server = b.add(
        Service::Cart,
        Some(call),
        SpanKind::Server,
        "GetCart",
        b.start(call) + hop,
        server,
        vec![kv("app.cart.items", int_val(b.pick(6, 0, 12) as i64))],
    );
    let get = b.add(
        Service::Cart,
        Some(cart_server),
        SpanKind::Client,
        "redis HGETALL",
        b.start(cart_server) + u / 10,
        first,
        vec![kv("db.system", str_val("redis"))],
    );
    let expire = b.add(
        Service::Cart,
        Some(cart_server),
        SpanKind::Client,
        "redis EXPIRE",
        b.end(get) + gap,
        second,
        vec![
            kv("db.system", str_val("redis")),
            kv("app.cache.ttl", double_val(1.5)),
        ],
    );
    match variant {
        ErrorVariant::None => {}
        ErrorVariant::Deep => {
            b.fail_origin(expire, "ConnectionReset");
            for idx in [cart_server, call, entry_idx, root] {
                b.fail_propagated(idx);
            }
        }
        ErrorVariant::Handled => {
            b.fail_origin(cart_server, "CartLocked");
            b.fail_propagated(call);
        }
        ErrorVariant::ExplicitOk => {
            b.set_status(entry_idx, StatusCode::Ok, "");
            b.set_status(cart_server, StatusCode::Ok, "");
        }
        ErrorVariant::Leaf => b.fail_origin(get, "KeyEvicted"),
    }
}

/// frontend POST /api/checkout → checkout PlaceOrder → currency Convert and
/// payment Charge (overlapping) → an order message consumed by accounting
/// after the request has ended (a child outliving its parent).
fn checkout(b: &mut TraceBuilder, t0: u64, variant: ErrorVariant) {
    let u = b.unit();
    let hop = u / 10 + 1;
    let before = b.pick(7, 1, 3) * u / 2;
    let convert_server = b.pick(8, 1, 4) * u;
    let convert = convert_server + 2 * hop;
    let validate = b.pick(9, 1, 5) * u;
    let charge_server = u / 10 + validate + u / 5;
    let charge = charge_server + 2 * hop;
    let charge_offset = u / 5 + convert / 2;
    let publish_offset = (u / 5 + convert).max(charge_offset + charge) + u / 10;
    let publish = u / 5;
    let place_order = publish_offset + publish + u / 5;
    let client = place_order + 2 * hop;
    let root_duration = before + client + u / 2;

    let failed_entry = matches!(variant, ErrorVariant::Deep);
    let attrs = http_entry_attributes(b, "POST /api/checkout", failed_entry);
    let root = b.add(
        Service::Frontend,
        None,
        SpanKind::Server,
        "POST /api/checkout",
        t0,
        root_duration,
        attrs,
    );
    let call = b.add(
        Service::Frontend,
        Some(root),
        SpanKind::Client,
        "PlaceOrder",
        t0 + before,
        client,
        vec![kv("rpc.system", str_val("grpc"))],
    );
    let order = b.add(
        Service::Checkout,
        Some(call),
        SpanKind::Server,
        "PlaceOrder",
        b.start(call) + hop,
        place_order,
        vec![kv("app.order.items", int_val(b.pick(10, 1, 9) as i64))],
    );
    let order_start = b.start(order);
    let convert_call = b.add(
        Service::Checkout,
        Some(order),
        SpanKind::Client,
        "Convert",
        order_start + u / 5,
        convert,
        vec![kv("rpc.system", str_val("grpc"))],
    );
    let fx = b.add(
        Service::Currency,
        Some(convert_call),
        SpanKind::Server,
        "Convert",
        b.start(convert_call) + hop,
        convert_server,
        vec![kv("app.currency.to", str_val("EUR"))],
    );
    let charge_call = b.add(
        Service::Checkout,
        Some(order),
        SpanKind::Client,
        "Charge",
        order_start + charge_offset,
        charge,
        vec![kv("rpc.system", str_val("grpc"))],
    );
    let pay = b.add(
        Service::Payment,
        Some(charge_call),
        SpanKind::Server,
        "Charge",
        b.start(charge_call) + hop,
        charge_server,
        vec![kv("app.payment.card_type", str_val("visa"))],
    );
    let check = b.add(
        Service::Payment,
        Some(pay),
        SpanKind::Internal,
        "validate card",
        b.start(pay) + u / 10,
        validate,
        vec![],
    );
    let produce = b.add(
        Service::Checkout,
        Some(order),
        SpanKind::Producer,
        "orders publish",
        order_start + publish_offset,
        publish,
        vec![
            kv("messaging.system", str_val("kafka")),
            kv("messaging.destination.name", str_val("orders")),
        ],
    );
    let consume_start = b.end(produce) + u;
    let consume_duration = b.pick(11, 2, 6) * u;
    let consume = b.add(
        Service::Accounting,
        Some(produce),
        SpanKind::Consumer,
        "orders process",
        consume_start,
        consume_duration,
        vec![kv("messaging.system", str_val("kafka"))],
    );
    let root_span_id = b.spans[root].span.span_id.clone();
    let trace = b.trace_id.clone();
    b.spans[consume].span.links.push(Link {
        trace_id: trace,
        span_id: root_span_id,
        trace_state: String::new(),
        attributes: vec![kv("link.reason", str_val("originating request"))],
        dropped_attributes_count: 0,
        flags: 0,
    });
    match variant {
        ErrorVariant::None => {}
        ErrorVariant::Deep => {
            b.fail_origin(check, "CardDeclined");
            for idx in [pay, charge_call, order, call, root] {
                b.fail_propagated(idx);
            }
        }
        ErrorVariant::Handled => {
            b.fail_origin(fx, "RateUnavailable");
            b.fail_propagated(convert_call);
        }
        ErrorVariant::ExplicitOk => {
            b.set_status(root, StatusCode::Ok, "");
            b.set_status(order, StatusCode::Ok, "");
            b.set_status(pay, StatusCode::Ok, "");
        }
        ErrorVariant::Leaf => b.fail_origin(consume, "LedgerWriteFailed"),
    }
}

/// A long streaming call whose caller never exports its span: the entry span
/// is an orphan with an inbound role and lands in the slowest band.
fn flag_stream(b: &mut TraceBuilder, t0: u64, variant: ErrorVariant) {
    let duration = b.pick(12, 12, 40) * 1_000_000_000;
    let resolve = b.pick(13, 1, 9) * 100_000;
    let stream = b.add(
        Service::Flagd,
        None,
        SpanKind::Server,
        "EventStream",
        t0,
        duration,
        vec![
            kv("rpc.system", str_val("grpc")),
            kv("app.flag.count", int_val(b.pick(14, 3, 30) as i64)),
        ],
    );
    b.spans[stream].span.parent_span_id = ORPHAN_PARENT_SPAN_ID.to_vec();
    let inner = b.add(
        Service::Flagd,
        Some(stream),
        SpanKind::Internal,
        "resolve flags",
        t0 + 1_000_000,
        resolve,
        vec![],
    );
    match variant {
        ErrorVariant::Deep => {
            b.fail_origin(inner, "FlagParseError");
            b.fail_propagated(stream);
        }
        ErrorVariant::Handled | ErrorVariant::Leaf => b.fail_origin(inner, "FlagMissing"),
        ErrorVariant::ExplicitOk => b.set_status(stream, StatusCode::Ok, ""),
        ErrorVariant::None => {}
    }
}

/// Deterministic edge cases on top of the shapes, by trace index:
/// - every 11th trace's first span is received twice;
/// - every 13th trace gets a second root;
/// - every 17th trace's last span has its end before its start (clock skew);
/// - every 19th trace's last span has zero length.
fn apply_edge_cases(b: &mut TraceBuilder) {
    let key = b.key();
    let last = b.spans.len() - 1;
    if key % 11 == 5 {
        b.resends.push((0, 1_000_000_000));
    }
    if key % 13 == 7 {
        let service = b.spans[0].service;
        let start = b.start(0);
        b.add(
            service,
            None,
            SpanKind::Internal,
            "background refresh",
            start,
            1_500_000,
            vec![],
        );
    }
    if key % 17 == 3 {
        let span = &mut b.spans[last].span;
        span.end_time_unix_nano = span.start_time_unix_nano.saturating_sub(1);
    } else if key % 19 == 4 {
        let span = &mut b.spans[last].span;
        span.end_time_unix_nano = span.start_time_unix_nano;
    }
}

fn kv(key: &str, value: Option<AnyValue>) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value,
    }
}

fn value(v: any_value::Value) -> Option<AnyValue> {
    Some(AnyValue { value: Some(v) })
}

fn str_val(s: &str) -> Option<AnyValue> {
    value(any_value::Value::StringValue(s.to_string()))
}

fn int_val(i: i64) -> Option<AnyValue> {
    value(any_value::Value::IntValue(i))
}

fn bool_val(b: bool) -> Option<AnyValue> {
    value(any_value::Value::BoolValue(b))
}

fn double_val(d: f64) -> Option<AnyValue> {
    value(any_value::Value::DoubleValue(d))
}

fn array_val(items: &[&str]) -> Option<AnyValue> {
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        values.push(AnyValue {
            value: Some(any_value::Value::StringValue(item.to_string())),
        });
    }
    value(any_value::Value::ArrayValue(ArrayValue { values }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn params(traces: usize, seed: u64) -> MeshParams {
        MeshParams {
            traces,
            start_ns: 1_700_000_000_000_000_000,
            trace_spacing_ns: 250_000_000,
            seed,
        }
    }

    fn is_error(span: &Span) -> bool {
        span.status
            .as_ref()
            .is_some_and(|s| s.code == StatusCode::Error as i32)
    }

    #[test]
    fn same_params_same_corpus() {
        assert_eq!(generate(&params(120, 7)), generate(&params(120, 7)));
    }

    #[test]
    fn distinct_seeds_have_disjoint_traces() {
        let a: BTreeSet<Vec<u8>> = generate(&params(80, 1))
            .into_iter()
            .map(|s| s.span.trace_id)
            .collect();
        let b: BTreeSet<Vec<u8>> = generate(&params(80, 2))
            .into_iter()
            .map(|s| s.span.trace_id)
            .collect();
        assert!(a.is_disjoint(&b));
    }

    #[test]
    fn arrival_order_follows_end_time_except_resends() {
        let corpus = generate(&params(200, 3));
        let mut seen = BTreeSet::new();
        let mut last_end = 0;
        for s in &corpus {
            let first_copy = seen.insert((s.span.trace_id.clone(), s.span.span_id.clone()));
            if first_copy {
                assert!(
                    s.span.end_time_unix_nano >= last_end
                        || s.span.end_time_unix_nano < s.span.start_time_unix_nano
                );
                last_end = last_end.max(s.span.end_time_unix_nano);
            }
        }
    }

    /// Fifty consecutive traces cover every shape, error variant and edge case
    /// the comparisons rely on.
    #[test]
    fn fifty_traces_cover_every_case() {
        let corpus = generate(&params(50, 0));

        let services: BTreeSet<Service> = corpus.iter().map(|s| s.service).collect();
        assert_eq!(services, Service::ALL.into_iter().collect());

        let kinds: BTreeSet<i32> = corpus.iter().map(|s| s.span.kind).collect();
        let want: BTreeSet<i32> = [
            SpanKind::Server,
            SpanKind::Client,
            SpanKind::Internal,
            SpanKind::Producer,
            SpanKind::Consumer,
        ]
        .into_iter()
        .map(|k| k as i32)
        .collect();
        assert_eq!(kinds, want);

        let mut by_trace: BTreeMap<Vec<u8>, Vec<&Span>> = BTreeMap::new();
        for s in &corpus {
            by_trace
                .entry(s.span.trace_id.clone())
                .or_default()
                .push(&s.span);
        }
        assert_eq!(by_trace.len(), 50);

        let mut resends = 0;
        let mut extra_roots = 0;
        let mut orphans = 0;
        let mut error_with_error_child = 0;
        let mut error_without_error_child = 0;
        let mut outlives_parent = 0;
        let mut overlapping_siblings = 0;
        let mut ok_status = 0;
        for spans in by_trace.values() {
            let ids: BTreeSet<&Vec<u8>> = spans.iter().map(|s| &s.span_id).collect();
            resends += spans.len() - ids.len();
            let mut roots = BTreeSet::new();
            for s in spans {
                if s.parent_span_id.is_empty() {
                    roots.insert(&s.span_id);
                } else if s.parent_span_id == ORPHAN_PARENT_SPAN_ID {
                    orphans += 1;
                } else {
                    assert!(ids.contains(&s.parent_span_id), "dangling parent");
                }
                if s.status
                    .as_ref()
                    .is_some_and(|st| st.code == StatusCode::Ok as i32)
                {
                    ok_status += 1;
                }
                let children: Vec<&&Span> = spans
                    .iter()
                    .filter(|c| c.parent_span_id == s.span_id)
                    .collect();
                if is_error(s) {
                    if children.iter().any(|c| is_error(c)) {
                        error_with_error_child += 1;
                    } else {
                        error_without_error_child += 1;
                    }
                }
                for c in &children {
                    if c.end_time_unix_nano > s.end_time_unix_nano {
                        outlives_parent += 1;
                    }
                }
                for (i, a) in children.iter().enumerate() {
                    for c in &children[i + 1..] {
                        let disjoint = a.end_time_unix_nano <= c.start_time_unix_nano
                            || c.end_time_unix_nano <= a.start_time_unix_nano;
                        if !disjoint {
                            overlapping_siblings += 1;
                        }
                    }
                }
            }
            if roots.len() > 1 {
                extra_roots += 1;
            }
        }
        assert!(resends > 0, "no resent span");
        assert!(extra_roots > 0, "no trace with two roots");
        assert!(orphans > 0, "no orphan");
        assert!(error_with_error_child > 0, "no propagated error");
        assert!(error_without_error_child > 0, "no error origin");
        assert!(outlives_parent > 0, "no child outliving its parent");
        assert!(overlapping_siblings > 0, "no overlapping siblings");
        assert!(ok_status > 0, "no explicit OK status");

        let skewed = corpus
            .iter()
            .filter(|s| s.span.end_time_unix_nano < s.span.start_time_unix_nano)
            .count();
        let zero = corpus
            .iter()
            .filter(|s| s.span.end_time_unix_nano == s.span.start_time_unix_nano)
            .count();
        assert!(skewed > 0, "no end-before-start span");
        assert!(zero > 0, "no zero-length span");

        let durations: Vec<u64> = corpus
            .iter()
            .map(|s| {
                s.span
                    .end_time_unix_nano
                    .saturating_sub(s.span.start_time_unix_nano)
            })
            .collect();
        assert!(
            durations.iter().any(|d| (1..1_000_000).contains(d)),
            "no sub-millisecond span"
        );
        assert!(
            durations.iter().any(|d| *d >= 10_000_000_000),
            "no span of ten seconds or more"
        );
    }

    #[test]
    fn requests_group_spans_by_service() {
        let corpus = generate(&params(30, 5));
        let requests = build_requests(&corpus, 40);
        let mut total = 0;
        for request in &requests {
            let mut names = BTreeSet::new();
            for rs in &request.resource_spans {
                let attrs = &rs.resource.as_ref().expect("resource").attributes;
                let name = attrs
                    .iter()
                    .find(|kv| kv.key == "service.name")
                    .and_then(|kv| kv.value.as_ref())
                    .and_then(|v| v.value.as_ref());
                let Some(any_value::Value::StringValue(name)) = name else {
                    panic!("service.name missing");
                };
                assert!(
                    names.insert(name.clone()),
                    "service repeated inside one request"
                );
                for ss in &rs.scope_spans {
                    total += ss.spans.len();
                }
            }
        }
        assert_eq!(total, corpus.len());
        assert_eq!(requests.len(), corpus.len().div_ceil(40));
    }
}
