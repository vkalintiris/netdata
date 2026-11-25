//! Pair detection: one model call can reach us as two call spans, and only one may count.
//!
//! - Nested: a wrapper library's span around the SDK's own span for the same call. Within one export request the
//!   inner span counts: a call span that another call span names as its parent is dropped. Across requests the first
//!   to arrive counts and finds the other in a short-lived cache. That is the child as a rule, because it ends first;
//!   a relay that sends its batches at once, or retries one, can deliver the parent first, and then the parent's
//!   values are the ones charted.
//! - Same answer: two call spans in different traces that carry the same response id (`gen_ai.response.id`) for the
//!   same service. The one that ended first counts.
//! - Same failure: the Anthropic SDK's own span does not join a wrapper library's trace (seen with OpenLLMetry), and a
//!   failed call has no response id. A failure without one from the SDK's scope and one from another scope, of the
//!   same service and requested model, one lying within the other in time, count once: the later to arrive is dropped.
//!
//! Only direct parents are checked: in every captured pair the inner span is a child of the outer one.

use crate::Call;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{BuildHasher, Hash, RandomState};
use std::mem;
use std::sync::Mutex;

/// How long, at least, a counted call is remembered for pairing with later requests, unless more than
/// [`MAX_REMEMBERED_CALLS`] arrive sooner; at most twice this.
const MEMORY_SECONDS: u64 = 60;

/// Call spans one half of the memory takes before the halves swap early. When calls arrive faster than this many in
/// [`MEMORY_SECONDS`], the memory holds the last one to two times this many of them: about 20 MB at most.
const MAX_REMEMBERED_CALLS: usize = 50_000;

/// Failures remembered per service, model and kind in each half of the memory; the oldest is forgotten first. A
/// storm of failures that find no partner would otherwise make every later failure search all of them.
const MAX_REMEMBERED_FAILURES: usize = 1000;

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

/// A span's key: its trace id and its span id.
type SpanKey = (u128, u64);

/// The key of span `span_id` of trace `trace_id`. `None` when an id does not have its OTLP length, 16 and 8 bytes
/// (a root span's parent id is empty): such a span takes no part in nesting.
fn span_key(trace_id: &[u8], span_id: &[u8]) -> Option<SpanKey> {
    let trace_id = u128::from_be_bytes(trace_id.try_into().ok()?);
    let span_id = u64::from_be_bytes(span_id.try_into().ok()?);
    Some((trace_id, span_id))
}

/// Remembers recently counted calls across export requests.
pub struct Pairs<G> {
    state: Mutex<State<G>>,
    /// Hashes a group: every remembered answer keeps the hash, not a copy of its group.
    hasher: RandomState,
}

struct State<G> {
    rotated_at: u64,
    /// How many times the halves swapped.
    generation: u64,
    current: Memory<G>,
    previous: Memory<G>,
}

struct Memory<G> {
    /// The parent every call span seen names: those parents must not count.
    parents: HashSet<SpanKey>,
    /// Every call span counted: their children must not count.
    counted: HashSet<SpanKey>,
    /// Every answer seen, by its group's hash and its response id.
    response_ids: HashSet<(u64, String)>,
    /// Call spans this half has taken.
    calls: usize,
    /// Counted failures without a response id, by service and requested model.
    failures: HashMap<(G, Option<String>), Failures>,
}

/// Counted failures of one service and model, oldest first, apart by whether their scope keeps its own trace: a
/// failure pairs only with one of the other kind, so it never searches its own.
#[derive(Default)]
struct Failures {
    own_trace: VecDeque<Failure>,
    joined: VecDeque<Failure>,
}

impl Failures {
    fn of(&mut self, own_trace: bool) -> &mut VecDeque<Failure> {
        if own_trace {
            &mut self.own_trace
        } else {
            &mut self.joined
        }
    }
}

/// A counted failure's time span.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Failure {
    start: u64,
    end: u64,
}

impl<G> Default for Memory<G> {
    fn default() -> Self {
        Self {
            parents: HashSet::new(),
            counted: HashSet::new(),
            response_ids: HashSet::new(),
            calls: 0,
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
                generation: 0,
                current: Memory::default(),
                previous: Memory::default(),
            }),
            hasher: RandomState::new(),
        }
    }

    /// The candidates of one export request that count, in their original order; `now` is Unix seconds.
    pub fn retain<T>(&self, now: u64, candidates: Vec<Candidate<G, T>>) -> Vec<Candidate<G, T>> {
        let (keep, _) = self.decide(now, &candidates);
        candidates
            .into_iter()
            .zip(keep)
            .filter_map(|(candidate, keep)| keep.then_some(candidate))
            .collect()
    }

    /// Whether each candidate of one export request counts, and what the memory took from the request; `now` is
    /// Unix seconds. A caller that then fails to store the request gives the second value to [`Self::forget`].
    pub fn decide<T>(
        &self,
        now: u64,
        candidates: &[Candidate<G, T>],
    ) -> (Vec<bool>, Remembered<G>) {
        let mut remembered = Remembered::default();
        if candidates.is_empty() {
            return (Vec::new(), remembered);
        }
        let mut state = self.state.lock().unwrap();
        if now.saturating_sub(state.rotated_at) >= MEMORY_SECONDS
            || state.current.calls >= MAX_REMEMBERED_CALLS
        {
            state.previous = mem::take(&mut state.current);
            state.rotated_at = now;
            state.generation += 1;
        }
        state.current.calls += candidates.len();
        remembered.generation = state.generation;
        remembered.calls = candidates.len();

        // Each candidate's own key and the key of the parent it names.
        let keys: Vec<(Option<SpanKey>, Option<SpanKey>)> = candidates
            .iter()
            .map(|candidate| {
                (
                    span_key(&candidate.trace_id, &candidate.span_id),
                    span_key(&candidate.trace_id, &candidate.parent_span_id),
                )
            })
            .collect();
        let parents: HashSet<SpanKey> = keys.iter().filter_map(|(_, parent)| *parent).collect();
        let mut keep: Vec<bool> = keys
            .iter()
            .map(|(own, parent)| {
                own.is_none_or(|own| !parents.contains(&own) && !state.remembers_parent(own))
                    && parent.is_none_or(|parent| !state.remembers_counted(parent))
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
            let key = (self.hasher.hash_one(&candidate.group), response_id.clone());
            if state.remembers_response(&key) || !seen.insert(key.clone()) {
                keep[index] = false;
            }
            if state.current.response_ids.insert(key.clone()) {
                remembered.response_ids.push(key);
            }
        }
        // A dropped parent's answer is remembered too, but only now: its own child carries the same id.
        for (candidate, keep) in candidates.iter().zip(&keep) {
            if let (false, Some(response_id)) = (*keep, &candidate.call.response_id) {
                let key = (self.hasher.hash_one(&candidate.group), response_id.clone());
                if state.current.response_ids.insert(key.clone()) {
                    remembered.response_ids.push(key);
                }
            }
        }

        for &index in &order {
            let candidate = &candidates[index];
            let call = &candidate.call;
            if !keep[index] || !call.error || call.response_id.is_some() {
                continue;
            }
            let end = call.end_time_unix_nano;
            let failure = Failure {
                start: end.saturating_sub((call.duration_seconds * 1e9) as u64),
                end,
            };
            let own_trace = candidate.scope.starts_with(OWN_TRACE_SCOPE_PREFIX);
            let key = (candidate.group.clone(), call.request_model.clone());
            if let Some((taken, current)) = state.take_failure(&key, &failure, own_trace) {
                keep[index] = false;
                remembered.failures_taken.push(TakenFailure {
                    key,
                    own_trace: !own_trace,
                    failure: taken,
                    current,
                });
            } else {
                let failures = state.current.failures.entry(key.clone()).or_default();
                let list = failures.of(own_trace);
                if list.len() == MAX_REMEMBERED_FAILURES {
                    list.pop_front();
                }
                list.push_back(failure);
                remembered.failures.push((key, own_trace, failure));
            }
        }

        for ((own, parent), keep) in keys.iter().zip(&keep) {
            if let Some(parent) = parent
                && state.current.parents.insert(*parent)
            {
                remembered.parents.push(*parent);
            }
            if let (true, Some(own)) = (*keep, own)
                && state.current.counted.insert(*own)
            {
                remembered.counted.push(*own);
            }
        }
        (keep, remembered)
    }

    /// Take out of the memory what [`Self::decide`] put in for a request that was not stored: the sender sends the
    /// request again, and its calls must find the memory as the first attempt found it. Another request decided in
    /// between keeps the decisions it took against that memory.
    pub fn forget(&self, remembered: Remembered<G>) {
        let mut state = self.state.lock().unwrap();
        let generation = state.generation;
        // A failure this request paired with goes back to the half it was in.
        for taken in remembered.failures_taken {
            let age = generation - remembered.generation + u64::from(!taken.current);
            if let Some(memory) = state.half(age) {
                let failures = memory.failures.entry(taken.key).or_default();
                failures.of(taken.own_trace).push_back(taken.failure);
            }
        }
        let Some(memory) = state.half(generation - remembered.generation) else {
            return;
        };
        memory.calls = memory.calls.saturating_sub(remembered.calls);
        for key in &remembered.parents {
            memory.parents.remove(key);
        }
        for key in &remembered.counted {
            memory.counted.remove(key);
        }
        for key in &remembered.response_ids {
            memory.response_ids.remove(key);
        }
        for (key, own_trace, failure) in remembered.failures {
            if let Some(failures) = memory.failures.get_mut(&key) {
                let list = failures.of(own_trace);
                if let Some(position) = list.iter().rposition(|other| *other == failure) {
                    list.remove(position);
                }
            }
        }
    }
}

/// What one export request added to the memory of a [`Pairs`], and the half it added it to.
pub struct Remembered<G> {
    /// The memory's generation when the request was decided: the current half's.
    generation: u64,
    calls: usize,
    parents: Vec<SpanKey>,
    counted: Vec<SpanKey>,
    response_ids: Vec<(u64, String)>,
    /// Failures added, by key and kind.
    failures: Vec<((G, Option<String>), bool, Failure)>,
    failures_taken: Vec<TakenFailure<G>>,
}

/// A remembered failure that a failure of the request paired with and so removed.
struct TakenFailure<G> {
    key: (G, Option<String>),
    /// The kind of the removed failure.
    own_trace: bool,
    failure: Failure,
    /// Whether it was in the current half; the previous one otherwise.
    current: bool,
}

impl<G> Default for Remembered<G> {
    fn default() -> Self {
        Self {
            generation: 0,
            calls: 0,
            parents: Vec::new(),
            counted: Vec::new(),
            response_ids: Vec::new(),
            failures: Vec::new(),
            failures_taken: Vec::new(),
        }
    }
}

impl<G> State<G> {
    /// The half that was current `age` rotations ago, while it is remembered.
    fn half(&mut self, age: u64) -> Option<&mut Memory<G>> {
        match age {
            0 => Some(&mut self.current),
            1 => Some(&mut self.previous),
            _ => None,
        }
    }
}

impl<G: Hash + Eq> State<G> {
    fn remembers_parent(&self, key: SpanKey) -> bool {
        self.current.parents.contains(&key) || self.previous.parents.contains(&key)
    }

    fn remembers_counted(&self, key: SpanKey) -> bool {
        self.current.counted.contains(&key) || self.previous.counted.contains(&key)
    }

    fn remembers_response(&self, key: &(u64, String)) -> bool {
        self.current.response_ids.contains(key) || self.previous.response_ids.contains(key)
    }

    /// Forgets and returns a remembered failure of the other kind that `failure` is the other half of, and whether
    /// it was in the current half.
    fn take_failure(
        &mut self,
        key: &(G, Option<String>),
        failure: &Failure,
        own_trace: bool,
    ) -> Option<(Failure, bool)> {
        let within = |a: &Failure, b: &Failure| a.start >= b.start && a.end <= b.end;
        for (memory, current) in [(&mut self.current, true), (&mut self.previous, false)] {
            let Some(failures) = memory.failures.get_mut(key) else {
                continue;
            };
            let others = failures.of(!own_trace);
            if let Some(position) = others
                .iter()
                .position(|other| within(other, failure) || within(failure, other))
            {
                return others.remove(position).map(|taken| (taken, current));
            }
        }
        None
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
                duration_seconds: 0.0,
                time_to_first_chunk_seconds: None,
                response_id: response_id.map(str::to_string),
            },
            trace_id: vec![trace; 16],
            span_id: vec![span; 8],
            parent_span_id: if parent == 0 {
                Vec::new()
            } else {
                vec![parent; 8]
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
    fn a_child_in_a_later_request_does_not_count() {
        // A relay that sends its batches at once can deliver the outer span first.
        let pairs = Pairs::new();
        assert_eq!(
            kept(&pairs.retain(100, vec![candidate(1, 0, 9, None, 20)])),
            vec![1]
        );
        assert_eq!(
            kept(&pairs.retain(130, vec![candidate(2, 1, 9, None, 10)])),
            Vec::<u8>::new()
        );
        // Forgotten after two rotations, like the rest of the memory.
        pairs.retain(190, vec![candidate(4, 0, 8, None, 40)]);
        assert_eq!(
            kept(&pairs.retain(250, vec![candidate(3, 1, 9, None, 15)])),
            vec![3]
        );
    }

    #[test]
    fn a_dropped_parent_does_not_cover_its_other_children() {
        // An outer span around two calls: both inner spans count, the outer one never does.
        let pairs = Pairs::new();
        assert_eq!(
            kept(&pairs.retain(100, vec![candidate(2, 1, 9, None, 10)])),
            vec![2]
        );
        assert!(
            pairs
                .retain(100, vec![candidate(1, 0, 9, None, 30)])
                .is_empty()
        );
        assert_eq!(
            kept(&pairs.retain(100, vec![candidate(3, 1, 9, None, 20)])),
            vec![3]
        );
    }

    #[test]
    fn spans_with_malformed_ids_do_not_nest() {
        // OTLP ids are 16 and 8 bytes; anything else cannot name a parent or be one.
        let pairs = Pairs::new();
        let mut outer = candidate(1, 0, 9, None, 20);
        let mut inner = candidate(2, 1, 9, None, 10);
        for span in [&mut outer, &mut inner] {
            span.trace_id.truncate(4);
        }
        assert_eq!(kept(&pairs.retain(100, vec![outer, inner])), vec![1, 2]);
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

    /// Whether each candidate counts, by its `data`.
    fn decided(
        pairs: &Pairs<&'static str>,
        now: u64,
        request: &[Candidate<&'static str, u8>],
    ) -> Vec<(u8, bool)> {
        let (keep, _) = pairs.decide(now, request);
        request
            .iter()
            .map(|candidate| candidate.data)
            .zip(keep)
            .collect()
    }

    #[test]
    fn a_forgotten_request_is_decided_again_the_same_way() {
        let sdk = "com.anthropic.sdk.messages";
        let request = || {
            vec![
                // A nested pair, an answer, and the SDK's half of a failure.
                candidate(1, 0, 9, None, 20),
                candidate(2, 1, 9, None, 10),
                candidate(3, 0, 8, Some("msg_1"), 30),
                failure(4, 7, sdk, 1_100, 1_900),
            ]
        };
        let pairs = Pairs::new();
        // The wrapper's half of the failure is remembered before the request.
        assert_eq!(
            decided(&pairs, 100, &[failure(5, 6, "wrapper", 1_000, 2_000)]),
            vec![(5, true)]
        );
        let expected = vec![(1, false), (2, true), (3, true), (4, false)];

        let (keep, remembered) = pairs.decide(100, &request());
        assert_eq!(keep, vec![false, true, true, false]);
        // Remembered, the same spans are duplicates of themselves.
        assert_eq!(decided(&pairs, 100, &request())[2], (3, false));
        let (_, again) = pairs.decide(100, &request());
        pairs.forget(again);
        pairs.forget(remembered);
        assert_eq!(decided(&pairs, 100, &request()), expected);
    }

    #[test]
    fn a_request_is_forgotten_after_the_memory_turned() {
        let pairs = Pairs::new();
        let (_, remembered) = pairs.decide(100, &[candidate(1, 0, 9, Some("msg_1"), 20)]);
        // The halves swap: the request is in the previous half now.
        assert_eq!(
            decided(&pairs, 160, &[candidate(2, 0, 8, Some("msg_2"), 30)]),
            vec![(2, true)]
        );
        pairs.forget(remembered);
        assert_eq!(
            decided(&pairs, 160, &[candidate(3, 0, 7, Some("msg_1"), 40)]),
            vec![(3, true)]
        );
        assert_eq!(
            decided(&pairs, 160, &[candidate(4, 0, 6, Some("msg_2"), 50)]),
            vec![(4, false)]
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
        candidate.call.duration_seconds = (end - start) as f64 / 1e9;
        candidate.scope = scope.to_string();
        candidate
    }

    #[test]
    fn the_memory_is_bounded_by_calls_too() {
        // As many unrelated calls as one half of the memory takes, all within one second.
        let flood = || -> Vec<_> {
            (0..MAX_REMEMBERED_CALLS)
                .map(|_| candidate(7, 0, 8, None, 5))
                .collect()
        };
        let pairs = Pairs::new();
        assert_eq!(
            kept(&pairs.retain(100, vec![candidate(2, 1, 9, None, 10)])),
            vec![2]
        );
        // One flood later the child is still remembered: its parent does not count.
        pairs.retain(100, flood());
        assert!(
            pairs
                .retain(100, vec![candidate(1, 0, 9, None, 20)])
                .is_empty()
        );
        // Two floods later it is forgotten.
        pairs.retain(100, flood());
        assert_eq!(
            kept(&pairs.retain(100, vec![candidate(1, 0, 9, None, 20)])),
            vec![1]
        );
    }

    #[test]
    fn remembered_failures_are_bounded() {
        let sdk = "com.anthropic.sdk.python";
        // One more wrapper failure than the memory holds, none with a partner yet; each lasts 100 ns.
        let pairs = Pairs::new();
        let storm: Vec<_> = (0..=MAX_REMEMBERED_FAILURES as u64)
            .map(|index| failure(1, 8, "wrapper", index * 1_000, index * 1_000 + 100))
            .collect();
        assert_eq!(pairs.retain(100, storm).len(), MAX_REMEMBERED_FAILURES + 1);
        // The oldest was forgotten: its partner counts. The next oldest is still there.
        assert_eq!(
            kept(&pairs.retain(100, vec![failure(2, 9, sdk, 10, 90)])),
            vec![2]
        );
        assert!(
            pairs
                .retain(100, vec![failure(3, 9, sdk, 1_010, 1_090)])
                .is_empty()
        );
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
