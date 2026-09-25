//! The typed, wire-neutral key vocabulary:
//! owners and keys are ENUMS, never grammar strings. Each wire adapter —
//! a future Netdata UI, the CLI — owns its own string rendering of this
//! vocabulary, in both directions, and must round-trip it; the engine
//! stays wire-neutral (no query grammar's words may leak in here). The
//! 4c predicate AST consumes these same enums.
//!
//! The vocabulary is OpenTelemetry's own: ATTRIBUTES belong to an owner
//! (the OTLP message carrying them — Resource, InstrumentationScope,
//! Span, Event, Link); BUILTIN FIELDS are the fixed members of those
//! messages themselves (name, kind, status, ...).
//!
//! The storage↔vocabulary mapping lives here in BOTH directions —
//! [`storage_to_attribute`] for key enumeration; [`AttributeOwner::attribute_prefix`]
//! and [`BuiltinField::dictionary_field`] for value lookups and 4c
//! lowering — so there is exactly one table to keep right.

/// Which OTLP message a key belongs to. `Resource`/`Span`/
/// `Instrumentation`/`Event`/`Link` hold attribute keys (prefix-stripped
/// storage names — the prefixes are artifacts of this crate family's
/// flattening, not something a consumer should see); `Builtin` holds the
/// fixed [`BuiltinField`] set; `Any` (predicates only) is the
/// owner-agnostic attribute, pinned as the resource ∪ span disjunction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttributeOwner {
    Resource,
    Span,
    Instrumentation,
    Event,
    Link,
    Builtin,
    Any,
}

impl AttributeOwner {
    /// The storage prefix this owner's attribute keys carry on disk;
    /// `None` for [`Builtin`](AttributeOwner::Builtin) (builtin fields
    /// are not attributes) and for [`Any`](AttributeOwner::Any) (no
    /// single prefix — predicate lowering expands it per owner).
    /// Stripping/prepending happens against the FULL prefix
    /// (`events.attributes.`, not `events.`) — an owner-word-only strip
    /// would leave a stray `attributes.` on every key.
    pub fn attribute_prefix(self) -> Option<&'static str> {
        match self {
            AttributeOwner::Resource => Some("resource.attributes."),
            AttributeOwner::Span => Some("attributes."),
            AttributeOwner::Instrumentation => Some("scope.attributes."),
            AttributeOwner::Event => Some("events.attributes."),
            AttributeOwner::Link => Some("links.attributes."),
            AttributeOwner::Builtin | AttributeOwner::Any => None,
        }
    }
}

/// The fixed builtin-field set — engine capabilities, not data
/// properties, which is why key enumeration lists ALL of them
/// unconditionally: filtering on `Status` is valid on a
/// corpus with no statuses (it matches nothing).
///
/// Dictionary-backed builtins ([`dictionary_field`]
/// (Self::dictionary_field) = `Some`) support value enumeration; the
/// rest are VIRTUAL — backed by per-row columns (`Duration`, `SpanId`,
/// `ParentSpanId`, `TraceId`), by the EVNB/LNKB structures (`LinkSpanId`,
/// `LinkTraceId`, `EventTimeSinceStart`), or by post-assembly derivation
/// (`RootName`, `RootServiceName`, `TraceDuration`) — and value
/// enumeration on them is a request error.
///
/// `trace_state` is deliberately NOT a builtin: the key
/// vocabulary exists for filter autocomplete, and no query path filters
/// on it; it stays visible in trace-by-id results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuiltinField {
    // ── Dictionary-backed ───────────────────────────────────────────
    Name,
    Kind,
    Status,
    StatusMessage,
    InstrumentationName,
    InstrumentationVersion,
    EventName,
    // ── Virtual ─────────────────────────────────────────────────────
    Duration,
    SpanId,
    ParentSpanId,
    TraceId,
    LinkSpanId,
    LinkTraceId,
    EventTimeSinceStart,
    RootName,
    RootServiceName,
    TraceDuration,
}

impl BuiltinField {
    /// Every builtin field, in declaration order (dictionary-backed first).
    pub const ALL: [BuiltinField; 17] = [
        BuiltinField::Name,
        BuiltinField::Kind,
        BuiltinField::Status,
        BuiltinField::StatusMessage,
        BuiltinField::InstrumentationName,
        BuiltinField::InstrumentationVersion,
        BuiltinField::EventName,
        BuiltinField::Duration,
        BuiltinField::SpanId,
        BuiltinField::ParentSpanId,
        BuiltinField::TraceId,
        BuiltinField::LinkSpanId,
        BuiltinField::LinkTraceId,
        BuiltinField::EventTimeSinceStart,
        BuiltinField::RootName,
        BuiltinField::RootServiceName,
        BuiltinField::TraceDuration,
    ];

    /// The storage dictionary field backing this builtin; `None` means
    /// VIRTUAL (no value dictionary anywhere — see the type docs).
    ///
    /// Values come back as the STORAGE labels (`kind` ∈
    /// `INTERNAL/SERVER/CLIENT/PRODUCER/CONSUMER`, `status` ∈
    /// `OK/ERROR`); mapping those to a wire vocabulary is the wire
    /// adapter's job, next to its name table.
    pub fn dictionary_field(self) -> Option<&'static str> {
        match self {
            BuiltinField::Name => Some("name"),
            BuiltinField::Kind => Some("kind"),
            BuiltinField::Status => Some("status_code"),
            BuiltinField::StatusMessage => Some("status_message"),
            BuiltinField::InstrumentationName => Some("scope.name"),
            BuiltinField::InstrumentationVersion => Some("scope.version"),
            BuiltinField::EventName => Some("events.name"),
            _ => None,
        }
    }
}

/// The resource `service.name` attribute's full storage field — the one
/// composite spelling several folds need (`summarize`, the trace
/// aggregates). Built from the vocabulary, never hand-written.
pub(crate) fn resource_service_field() -> String {
    format!(
        "{}service.name",
        AttributeOwner::Resource
            .attribute_prefix()
            .expect("Resource is an attribute owner")
    )
}

/// A span's resolved facet value by storage field name, borrowed (no
/// per-lookup allocation — hot-loop callers check e.g. `== "ERROR"`).
pub(crate) fn span_field<'a>(span: &'a sfst::TraceSpan, field: &str) -> Option<&'a str> {
    span.fields
        .iter()
        .find(|(k, _)| k == field)
        .map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The storage spelling of every attribute owner and dictionary-backed
    /// builtin, pinned.
    #[test]
    fn storage_spellings_are_pinned() {
        for (owner, prefix) in [
            (AttributeOwner::Resource, "resource.attributes."),
            (AttributeOwner::Span, "attributes."),
            (AttributeOwner::Instrumentation, "scope.attributes."),
            (AttributeOwner::Event, "events.attributes."),
            (AttributeOwner::Link, "links.attributes."),
        ] {
            assert_eq!(owner.attribute_prefix(), Some(prefix), "{owner:?}");
        }
        for (builtin, storage) in [
            (BuiltinField::Name, "name"),
            (BuiltinField::Kind, "kind"),
            (BuiltinField::Status, "status_code"),
            (BuiltinField::StatusMessage, "status_message"),
            (BuiltinField::InstrumentationName, "scope.name"),
            (BuiltinField::InstrumentationVersion, "scope.version"),
            (BuiltinField::EventName, "events.name"),
        ] {
            assert_eq!(builtin.dictionary_field(), Some(storage), "{builtin:?}");
        }
    }

    /// The virtual/dictionary split is total and matches the pinned
    /// sets; ALL enumerates every variant exactly once.
    #[test]
    fn builtin_split_is_pinned() {
        use BuiltinField::*;
        let dictionary: Vec<BuiltinField> = BuiltinField::ALL
            .into_iter()
            .filter(|i| i.dictionary_field().is_some())
            .collect();
        assert_eq!(
            dictionary,
            vec![
                Name,
                Kind,
                Status,
                StatusMessage,
                InstrumentationName,
                InstrumentationVersion,
                EventName
            ]
        );
        let mut all = BuiltinField::ALL.to_vec();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), BuiltinField::ALL.len(), "ALL has duplicates");
    }
}
