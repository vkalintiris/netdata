//! Pair detection: one model call can reach us as two call spans, and only one may count.
//!
//! - Nested: a wrapper library's span around the SDK's own span for the same call. The inner span counts, so a call
//!   span that another call span names as its parent is dropped. A child ends first and so is exported no later than
//!   its parent: within one export request the pair is seen together, and across requests the parent finds its
//!   child in a short-lived cache. A parent exported before its child counts twice.
//! - Same answer: two call spans in different traces that carry the same response id (`gen_ai.response.id`) for the
//!   same service. The one that ended first counts.
//!
//! Only direct parents are checked: in every captured pair the inner span is a child of the outer one.

use crate::Call;
use std::collections::HashSet;
use std::hash::Hash;
use std::mem;
use std::sync::Mutex;

/// How long, at least, a counted call is remembered for pairing with later requests; at most twice this.
const MEMORY_SECONDS: u64 = 60;

/// A call span of one export request, with what pair detection needs to know about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate<G, T> {
    pub call: Call,
    pub trace_id: Vec<u8>,
    pub span_id: Vec<u8>,
    /// Empty for a root span.
    pub parent_span_id: Vec<u8>,
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
}

impl<G> Default for Memory<G> {
    fn default() -> Self {
        Self {
            parents: HashSet::new(),
            response_ids: HashSet::new(),
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
        for index in order {
            let candidate = &candidates[index];
            let Some(response_id) = &candidate.call.response_id else {
                continue;
            };
            let key = (candidate.group.clone(), response_id.clone());
            if !keep[index] {
                state.current.response_ids.insert(key);
                continue;
            }
            if state.remembers_response(&key) || !seen.insert(key.clone()) {
                keep[index] = false;
            }
            state.current.response_ids.insert(key);
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
}
