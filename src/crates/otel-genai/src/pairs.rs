//! Pair detection: one model call can reach us as two call spans, and only one may count.
//!
//! - Nested: a wrapper library's span around the SDK's own span for the same call. The inner span counts, so a call
//!   span that another call span names as its parent is dropped. A child ends first and so is exported no later than
//!   its parent: within one export request the pair is seen together, and across requests the parent finds its
//!   child in a short-lived cache. A parent exported before its child counts twice.
//! - Same answer: two call spans in different traces that carry the same response id (`gen_ai.response.id`) for the
//!   same service. The one that ended first counts.
//! - Same failure: the Anthropic SDK's own span does not join a wrapper library's trace (seen with OpenLLMetry), and a
//!   failed call has no response id. A failure without one from the SDK's scope and one from another scope, of the
//!   same service and requested model, one lying within the other in time, count once: the later to arrive is dropped.
//!
//! Only direct parents are checked: in every captured pair the inner span is a child of the outer one.

use crate::Call;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::mem;
use std::sync::Mutex;

/// How long, at least, a counted call is remembered for pairing with later requests; at most twice this.
const MEMORY_SECONDS: u64 = 60;

/// Scopes whose spans keep their own trace beside a wrapper library's span of the same call.
const OWN_TRACE_SCOPE_PREFIX: &str = "com.anthropic.sdk.";

/// A call span of one export request, with what pair detection needs to know about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate<G, T> {
    pub call: Call,
    pub trace_id: Vec<u8>,
    pub span_id: Vec<u8>,
    /// Empty for a root span.
    pub parent_span_id: Vec<u8>,
    /// The instrumentation scope's name.
    pub scope: String,
    /// What a response id is unique within, for example the tenant and service.
    pub group: G,
    /// Anything the caller carries along with the call.
    pub data: T,
}

/// Remembers recently counted calls across export requests.
pub struct Pairs<G> {
    state: Mutex<State<G>>,
}

struct State<G> {
    rotated_at: u64,
    current: Memory<G>,
    previous: Memory<G>,
}

struct Memory<G> {
    /// `(trace id, parent span id)` of every call span seen: their parents must not count.
    parents: HashSet<(Vec<u8>, Vec<u8>)>,
    response_ids: HashSet<(G, String)>,
    /// Counted failures without a response id, by service and requested model.
    failures: HashMap<(G, Option<String>), Vec<Failure>>,
}

/// A counted failure's time span, and whether it came from a scope that keeps its own trace.
struct Failure {
    start: u64,
    end: u64,
    own_trace: bool,
}

impl<G> Default for Memory<G> {
    fn default() -> Self {
        Self {
            parents: HashSet::new(),
            response_ids: HashSet::new(),
            failures: HashMap::new(),
        }
    }
}

impl<G: Hash + Eq + Clone> Default for Pairs<G> {
    fn default() -> Self {
        Self::new()
    }
}

impl<G: Hash + Eq + Clone> Pairs<G> {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                rotated_at: 0,
                current: Memory::default(),
                previous: Memory::default(),
            }),
        }
    }

    /// The candidates of one export request that count, in their original order; `now` is Unix seconds.
    pub fn retain<T>(&self, now: u64, candidates: Vec<Candidate<G, T>>) -> Vec<Candidate<G, T>> {
        if candidates.is_empty() {
            return candidates;
        }
        let mut state = self.state.lock().unwrap();
        if now.saturating_sub(state.rotated_at) >= MEMORY_SECONDS {
            state.previous = mem::take(&mut state.current);
            state.rotated_at = now;
        }

        let parents: HashSet<(&[u8], &[u8])> = candidates
            .iter()
            .filter(|candidate| !candidate.parent_span_id.is_empty())
            .map(|candidate| {
                (
                    candidate.trace_id.as_slice(),
                    candidate.parent_span_id.as_slice(),
                )
            })
            .collect();
        let mut keep: Vec<bool> = candidates
            .iter()
            .map(|candidate| {
                let key = (candidate.trace_id.as_slice(), candidate.span_id.as_slice());
                !parents.contains(&key)
                    && !state.remembers_parent(&candidate.trace_id, &candidate.span_id)
            })
            .collect();

        // The same answer counts once: the call that ended first wins.
        let mut order: Vec<usize> = (0..candidates.len()).collect();
        order.sort_by_key(|&index| candidates[index].call.end_time_unix_nano);
        let mut seen = HashSet::new();
        for &index in &order {
            let candidate = &candidates[index];
            let Some(response_id) = &candidate.call.response_id else {
                continue;
            };
            if !keep[index] {
                continue;
            }
            let key = (candidate.group.clone(), response_id.clone());
            if state.remembers_response(&key) || !seen.insert(key.clone()) {
                keep[index] = false;
            }
            state.current.response_ids.insert(key);
        }
        // A dropped parent's answer is remembered too, but only now: its own child carries the same id.
        for (candidate, keep) in candidates.iter().zip(&keep) {
            if let (false, Some(response_id)) = (*keep, &candidate.call.response_id) {
                let key = (candidate.group.clone(), response_id.clone());
                state.current.response_ids.insert(key);
            }
        }

        for &index in &order {
            let candidate = &candidates[index];
            let call = &candidate.call;
            if !keep[index] || !call.error || call.response_id.is_some() {
                continue;
            }
            let Some(duration) = call.duration_seconds else {
                continue;
            };
            let end = call.end_time_unix_nano;
            let failure = Failure {
                start: end.saturating_sub((duration * 1e9) as u64),
                end,
                own_trace: candidate.scope.starts_with(OWN_TRACE_SCOPE_PREFIX),
            };
            let key = (candidate.group.clone(), call.request_model.clone());
            if state.take_failure(&key, &failure) {
                keep[index] = false;
            } else {
                state.current.failures.entry(key).or_default().push(failure);
            }
        }

        for candidate in &candidates {
            if !candidate.parent_span_id.is_empty() {
                state
                    .current
                    .parents
                    .insert((candidate.trace_id.clone(), candidate.parent_span_id.clone()));
            }
        }
        drop(state);

        candidates
            .into_iter()
            .zip(keep)
            .filter_map(|(candidate, keep)| keep.then_some(candidate))
            .collect()
    }
}

impl<G: Hash + Eq> State<G> {
    fn remembers_parent(&self, trace_id: &[u8], span_id: &[u8]) -> bool {
        let key = (trace_id.to_vec(), span_id.to_vec());
        self.current.parents.contains(&key) || self.previous.parents.contains(&key)
    }

    fn remembers_response(&self, key: &(G, String)) -> bool {
        self.current.response_ids.contains(key) || self.previous.response_ids.contains(key)
    }

    /// Forgets and reports a remembered failure that `failure` is the other half of.
    fn take_failure(&mut self, key: &(G, Option<String>), failure: &Failure) -> bool {
        let within = |a: &Failure, b: &Failure| a.start >= b.start && a.end <= b.end;
        for memory in [&mut self.current, &mut self.previous] {
            let Some(failures) = memory.failures.get_mut(key) else {
                continue;
            };
            if let Some(position) = failures.iter().position(|other| {
                other.own_trace != failure.own_trace
                    && (within(other, failure) || within(failure, other))
            }) {
                failures.swap_remove(position);
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tokens;

    fn candidate(
        span: u8,
        parent: u8,
        trace: u8,
        response_id: Option<&str>,
        end: u64,
    ) -> Candidate<&'static str, u8> {
        Candidate {
            call: Call {
                operation: "chat".into(),
                provider: None,
                request_model: None,
                response_model: None,
                tokens: Tokens::default(),
                error: false,
                end_time_unix_nano: end,
                duration_seconds: None,
                response_id: response_id.map(str::to_string),
            },
            trace_id: vec![trace],
            span_id: vec![span],
            parent_span_id: if parent == 0 {
                Vec::new()
            } else {
                vec![parent]
            },
            scope: String::new(),
            group: "service",
            data: span,
        }
    }

    fn kept(candidates: &[Candidate<&'static str, u8>]) -> Vec<u8> {
        candidates.iter().map(|candidate| candidate.data).collect()
    }

    #[test]
    fn inner_span_wins_within_a_request() {
        let pairs = Pairs::new();
        let outer = candidate(1, 0, 9, None, 20);
        let inner = candidate(2, 1, 9, None, 10);
        let unrelated = candidate(3, 0, 8, None, 30);
        assert_eq!(
            kept(&pairs.retain(100, vec![outer, inner, unrelated])),
            vec![2, 3]
        );
    }

    #[test]
    fn a_parent_in_a_later_request_does_not_count() {
        let pairs = Pairs::new();
        assert_eq!(
            kept(&pairs.retain(100, vec![candidate(2, 1, 9, None, 10)])),
            vec![2]
        );
        assert_eq!(
            kept(&pairs.retain(130, vec![candidate(1, 0, 9, None, 20)])),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn the_same_answer_counts_once() {
        let pairs = Pairs::new();
        let first = candidate(1, 0, 9, Some("msg_1"), 10);
        let second = candidate(2, 0, 8, Some("msg_1"), 12);
        assert_eq!(kept(&pairs.retain(100, vec![second, first])), vec![1]);
        // A later request with the same answer does not count either; another service's does.
        assert_eq!(
            kept(&pairs.retain(110, vec![candidate(3, 0, 7, Some("msg_1"), 30)])),
            Vec::<u8>::new()
        );
        let mut other = candidate(4, 0, 6, Some("msg_1"), 40);
        other.group = "other service";
        assert_eq!(kept(&pairs.retain(110, vec![other])), vec![4]);
    }

    #[test]
    fn a_nested_pair_with_one_answer_keeps_the_inner_span() {
        // Wrapper and SDK span carry the same answer id and end in the same instant, the wrapper first in the request.
        let pairs = Pairs::new();
        let outer = candidate(1, 0, 9, Some("msg_1"), 10);
        let inner = candidate(2, 1, 9, Some("msg_1"), 10);
        assert_eq!(kept(&pairs.retain(100, vec![outer, inner])), vec![2]);
        // The answer stays remembered: the same id in a later request does not count.
        assert_eq!(
            kept(&pairs.retain(110, vec![candidate(3, 0, 7, Some("msg_1"), 30)])),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn memory_fades() {
        let pairs = Pairs::new();
        pairs.retain(100, vec![candidate(2, 1, 9, Some("msg_1"), 10)]);
        // Still remembered one rotation later, forgotten after two.
        assert!(
            pairs
                .retain(160, vec![candidate(1, 0, 9, None, 20)])
                .is_empty()
        );
        assert_eq!(
            kept(&pairs.retain(220, vec![candidate(5, 0, 7, Some("msg_1"), 30)])),
            vec![5]
        );
    }

    fn failure(
        span: u8,
        trace: u8,
        scope: &str,
        start: u64,
        end: u64,
    ) -> Candidate<&'static str, u8> {
        let mut candidate = candidate(span, 0, trace, None, end);
        candidate.call.error = true;
        candidate.call.duration_seconds = Some((end - start) as f64 / 1e9);
        candidate.scope = scope.to_string();
        candidate
    }

    #[test]
    fn a_failure_seen_by_the_sdk_and_a_wrapper_counts_once() {
        let sdk = "com.anthropic.sdk.python";
        // The SDK's span lies within the wrapper's, in another trace; either may arrive first.
        let pairs = Pairs::new();
        assert_eq!(
            kept(&pairs.retain(100, vec![failure(1, 9, sdk, 1_100, 1_900)])),
            vec![1]
        );
        assert!(
            pairs
                .retain(100, vec![failure(2, 8, "wrapper", 1_000, 2_000)])
                .is_empty()
        );
        let pairs = Pairs::new();
        assert_eq!(
            kept(&pairs.retain(
                100,
                vec![
                    failure(2, 8, "wrapper", 1_000, 2_000),
                    failure(1, 9, sdk, 1_100, 1_900)
                ]
            )),
            vec![1]
        );
        // Two wrapper failures, or ones that do not nest in time, stay apart.
        let pairs = Pairs::new();
        assert_eq!(
            kept(&pairs.retain(
                100,
                vec![
                    failure(1, 9, "wrapper", 1_100, 1_900),
                    failure(2, 8, "wrapper", 1_000, 2_000)
                ]
            )),
            vec![1, 2]
        );
        let pairs = Pairs::new();
        assert_eq!(
            kept(&pairs.retain(
                100,
                vec![
                    failure(1, 9, sdk, 1_100, 2_100),
                    failure(2, 8, "wrapper", 1_000, 2_000)
                ]
            )),
            vec![1, 2]
        );
    }
}
