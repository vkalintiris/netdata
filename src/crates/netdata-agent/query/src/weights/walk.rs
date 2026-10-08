//! The walk of a weights request over its hosts, their contexts and their metrics
//! (`src/web/api/queries/weights.c`: `query_scope_foreach_host_parallel()`, `query_weights_worker_thread()`,
//! `weights_do_node_callback()`, `weights_do_context_callback()`, `weights_for_rrdmetric()`).
//!
//! C spreads the hosts over worker threads when the agent has two CPUs or more and the request two hosts or more
//! to work on. The workers take contiguous blocks of the hosts and their results, statistics and counts are
//! merged in order, so one thread gives the same answer; this walk is that one thread, with the checks a worker
//! makes before each host and the sums of the hosts' versions as C's two passes leave them.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Instant;

use netdata_agent_rrd::contexts::Context;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::simple_pattern::SimplePattern;
use netdata_agent_web::progress::Tracker;

use super::Method;
use super::methods::Run;
use super::results::Of;
use crate::target::{
    MetricFilters, Versions, context_retention_matches, foreach_context, foreach_host, foreach_metric_in_context,
    host_matches,
};

/// What a weights request selects by, compiled once (`qwd`'s patterns): the hosts, the contexts (`scope_contexts`
/// also as given: it may name one context exactly), and the instances, labels, alerts and dimensions of the metric
/// walk, which gets no label-key pattern, both matching flags and the request's version.
#[derive(Default)]
pub struct Scope {
    pub scope_nodes: Option<SimplePattern>,
    pub nodes: Option<SimplePattern>,
    pub scope_contexts: Option<Vec<u8>>,
    pub scope_contexts_sp: Option<SimplePattern>,
    pub contexts_sp: Option<SimplePattern>,
    pub metrics: MetricFilters,
}

/// A weights request walking: the methods' run, the stop checks and what they found, the count of the metrics
/// looked at, and the sums of the hosts' versions.
pub struct Walk<'a> {
    pub run: Run<'a>,
    /// When the engine started (`qwd->timings.received_ut`) and how long the request may take.
    pub received: Instant,
    pub timeout_us: u128,
    /// The client went away (`qwr->interrupt_callback`); it leaves the errno of its socket peek behind.
    pub interrupted: &'a dyn Fn(&mut i32) -> bool,
    /// The request's progress row (`qwr->transaction`): one step per metric done. No finish line is ever set.
    pub progress: Option<Tracker<'a>>,
    /// `examined_dimensions`.
    pub examined: usize,
    pub timed_out: bool,
    pub was_interrupted: bool,
    /// `qwd->versions`: zeros unless the hosts were walked.
    pub versions: Versions,
}

impl Walk<'_> {
    fn stopped(&self) -> bool {
        self.timed_out || self.was_interrupted
    }

    fn client_went_away(&self) -> bool {
        let mut errno = 0;
        (self.interrupted)(&mut errno)
    }

    fn elapsed_us(&self) -> u128 {
        self.received.elapsed().as_micros()
    }

    /// `weights_for_rrdmetric()`: nothing for a stopped walk or a client that went away; else the metric counts
    /// as looked at and the method runs. A request that is over its time after that stops there, with this
    /// metric's result in; otherwise the progress moves one step.
    fn metric(&mut self, of: &Of) -> ControlFlow<()> {
        if self.stopped() {
            return ControlFlow::Break(());
        }
        if self.client_went_away() {
            self.was_interrupted = true;
            return ControlFlow::Break(());
        }
        self.examined += 1;
        self.run.metric(of);
        if self.elapsed_us() > self.timeout_us {
            self.timed_out = true;
            return ControlFlow::Break(());
        }
        if let Some(progress) = &self.progress {
            progress.step();
        }
        ControlFlow::Continue(())
    }

    /// `weights_do_context_callback()`: a context the request does not select is passed; so is one whose
    /// retention misses the highlighted window or, for the two methods that compare with a baseline, the
    /// baseline. Then its metrics, as the request's patterns leave them.
    fn context(&mut self, scope: &Scope, host: &Arc<Host>, hostname: &str, rc: &Arc<Context>) -> ControlFlow<()> {
        let run = &self.run;
        let highlighted = context_retention_matches(rc, run.after, run.before);
        let has_retention = match run.method {
            Method::Value | Method::AnomalyRate => highlighted,
            Method::Ks2 | Method::Volume => {
                highlighted && context_retention_matches(rc, run.baseline_after, run.baseline_before)
            }
        };
        if !has_retention {
            return ControlFlow::Continue(());
        }
        foreach_metric_in_context(host, rc, &scope.metrics, |instance, metric| {
            self.metric(&Of { host, hostname, context: rc, instance, metric })
        })
    }

    /// `weights_do_node_callback()`: the contexts of a host the request selects; its name is taken once, for
    /// every result of the host.
    fn node(&mut self, scope: &Scope, host: &Arc<Host>, queryable: bool) -> ControlFlow<()> {
        if !queryable {
            return ControlFlow::Continue(());
        }
        let hostname = host.hostname();
        let (text, scope_sp, contexts_sp) =
            (scope.scope_contexts.as_deref(), scope.scope_contexts_sp.as_ref(), scope.contexts_sp.as_ref());
        foreach_context(host, text, scope_sp, contexts_sp, queryable, |rc, queryable_context| {
            if queryable_context { self.context(scope, host, &hostname, rc) } else { ControlFlow::Continue(()) }
        })
    }

    /// `query_scope_foreach_host_parallel()` as one thread. (C also has a branch for a version-1 request with a
    /// routed host, which walks that host alone; its handler never gives a version-1 request a host, so the branch
    /// is dead and not ported.) A first walk over the hosts in scope sums their
    /// versions and lists the ones to work on. With one CPU, or fewer than two such hosts, C walks the hosts
    /// again and that walk's sums replace the first's. Otherwise each listed host is checked as a worker checks
    /// it (a stopped walk, the time, the client, the node patterns again) and adds its versions once more before
    /// its work.
    pub fn hosts(&mut self, scope: &Scope, hosts: &[Arc<Host>], cpus: usize) {
        let (scope_nodes, nodes) = (scope.scope_nodes.as_ref(), scope.nodes.as_ref());
        let mut versions = Versions::default();
        let mut listed = Vec::new();
        let _: ControlFlow<()> = foreach_host(hosts, scope_nodes, nodes, &mut versions, |host, queryable| {
            if queryable {
                listed.push(Arc::clone(host));
            }
            ControlFlow::Continue(())
        });
        self.versions = versions;
        if cpus < 2 || listed.len() < 2 {
            let mut versions = Versions::default();
            let _ = foreach_host(hosts, scope_nodes, nodes, &mut versions, |host, queryable| {
                self.node(scope, host, queryable)
            });
            self.versions = versions;
            return;
        }
        for host in &listed {
            if self.stopped() {
                break;
            }
            if self.elapsed_us() > self.timeout_us {
                self.timed_out = true;
                break;
            }
            if self.client_went_away() {
                self.was_interrupted = true;
                break;
            }
            if scope_nodes.is_some_and(|sp| !host_matches(sp, host)) {
                continue;
            }
            let queryable = nodes.is_none_or(|sp| host_matches(sp, host));
            self.versions.add_host(host);
            if self.node(scope, host, queryable).is_break() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use super::super::methods::{QueryEnv, Stats};
    use super::*;
    use crate::grouping::Windows;
    use crate::request::Profile;
    use crate::tables::{TimeGrouping, options};
    use crate::testing::{T0, W_NOW, W_POINTS, weights_host_as};

    const NEVER: &dyn Fn(&mut i32) -> bool = &|_| false;

    /// A walk of the `value` method over the fixture's last minute, with zeros kept, an hour to finish and a
    /// client that stays.
    fn walk(profile: &Profile) -> Walk<'_> {
        let last = T0 + W_POINTS;
        Walk {
            run: Run {
                env: QueryEnv { profile, windows: Windows::default(), queries: None, now_s: W_NOW },
                method: Method::Value,
                after: last - 60,
                before: last,
                baseline_after: last - 180,
                baseline_before: last - 60,
                points: 30,
                // (what the request's parser always adds)
                options: options::NOT_ALIGNED | options::NULL2ZERO,
                time_group: TimeGrouping::Average,
                time_group_options: None,
                tier: 0,
                shifts: 0,
                register_zero: true,
                stats: Stats::default(),
                results: Vec::new(),
            },
            received: Instant::now(),
            timeout_us: Duration::from_secs(3600).as_micros(),
            interrupted: NEVER,
            progress: None,
            examined: 0,
            timed_out: false,
            was_interrupted: false,
            versions: Versions::default(),
        }
    }

    fn sp(text: &str) -> Option<SimplePattern> {
        SimplePattern::from_web(text.as_bytes())
    }

    /// The request's version and both matching flags, as the engine sets them for the metric walk.
    fn scope(set: impl Fn(&mut Scope)) -> Scope {
        let metrics = MetricFilters { version: 2, match_ids: true, match_names: true, ..MetricFilters::default() };
        let mut scope = Scope { metrics, ..Scope::default() };
        set(&mut scope);
        scope
    }

    fn found(walk: &Walk) -> Vec<String> {
        walk.run.results.iter().map(|t| format!("{}:{}", t.hostname, t.metric.id())).collect()
    }

    fn all_of(hostname: &str) -> Vec<String> {
        ["a", "b", "z", "step"].iter().map(|dimension| format!("{hostname}:{dimension}")).collect()
    }

    /// The walk's order and what it selects by: hosts in the list's order, each host's metrics in theirs; the
    /// hidden metric is looked at and has no value; `scope_nodes` and `nodes` by name or by machine GUID; the
    /// context by its exact id, by a pattern, and by `contexts`; a dimensions pattern.
    #[test]
    fn the_walk_visits_the_metrics_the_request_selects() {
        let profile = Profile::default();
        let hosts = [weights_host_as("guid-1", "one"), weights_host_as("guid-2", "two")];
        let walked = |scope: &Scope| {
            let mut walk = walk(&profile);
            walk.hosts(scope, &hosts, 16);
            (found(&walk), walk.examined)
        };
        let both: Vec<String> = all_of("one").into_iter().chain(all_of("two")).collect();
        assert_eq!(walked(&scope(|_| {})), (both.clone(), 10));
        assert_eq!(walked(&scope(|s| s.scope_nodes = sp("two"))), (all_of("two"), 5));
        assert_eq!(walked(&scope(|s| s.nodes = sp("guid-1"))), (all_of("one"), 5));
        assert_eq!(walked(&scope(|s| s.nodes = sp("nomatch"))), (vec![], 0));
        // the context: named exactly, by a pattern, and by `contexts`
        let exact = |s: &mut Scope| {
            s.scope_contexts = Some(b"ctx.w".to_vec());
            s.scope_contexts_sp = sp("ctx.w");
        };
        assert_eq!(walked(&scope(exact)).1, 10);
        assert_eq!(walked(&scope(|s| s.scope_contexts_sp = sp("ctx.*"))).1, 10);
        assert_eq!(walked(&scope(|s| s.scope_contexts_sp = sp("other.*"))).1, 0);
        assert_eq!(walked(&scope(|s| s.contexts_sp = sp("!ctx.w|*"))).1, 0);
        // the metric walk's own patterns
        let b_alone = vec!["one:b".to_string(), "two:b".to_string()];
        assert_eq!(walked(&scope(|s| s.metrics.dimensions = sp("b"))), (b_alone, 2));
        assert_eq!(walked(&scope(|s| s.metrics.scope_instances = sp("t.w@guid-2"))), (all_of("two"), 5));
    }

    /// The retention gate of a context: the highlighted window alone for a value; the baseline too for the
    /// methods that compare with one.
    #[test]
    fn a_context_is_walked_when_its_retention_meets_the_windows() {
        let profile = Profile::default();
        let hosts = [weights_host_as("guid-1", "one")];
        let examined = |method: Method, highlighted: (i64, i64), baseline: (i64, i64)| {
            let mut walk = walk(&profile);
            walk.run.method = method;
            (walk.run.after, walk.run.before) = highlighted;
            (walk.run.baseline_after, walk.run.baseline_before) = baseline;
            walk.hosts(&scope(|_| {}), &hosts, 1);
            walk.examined
        };
        let last = T0 + W_POINTS;
        let (inside, long_before) = ((last - 60, last), (T0 - 100_000, T0 - 90_000));
        for method in [Method::Value, Method::AnomalyRate] {
            assert_eq!(examined(method, inside, long_before), 5, "{method:?}");
            assert_eq!(examined(method, long_before, inside), 0, "{method:?}");
        }
        for method in [Method::Ks2, Method::Volume] {
            assert_eq!(examined(method, inside, (last - 180, last - 60)), 5, "{method:?}");
            assert_eq!(examined(method, inside, long_before), 0, "{method:?}");
            assert_eq!(examined(method, long_before, inside), 0, "{method:?}");
        }
    }

    /// The sums of the hosts' versions, as C's passes leave them. S: the hosts `scope_nodes` selects; Q: those
    /// `nodes` selects too. With two CPUs or more and two hosts or more in Q: the sum over S plus the sum over Q
    /// (each worker adds its hosts' again); otherwise the sum over S alone (the second walk's sums replace the
    /// first's).
    #[test]
    fn the_versions_are_summed_as_c_s_passes_sum_them() {
        let profile = Profile::default();
        let hosts =
            [weights_host_as("guid-1", "one"), weights_host_as("guid-2", "two"), weights_host_as("guid-3", "three")];
        let version = |i: usize| u64::from(hosts[i].contexts().version());
        assert!(version(0) > 0);
        let summed = |scope: &Scope, cpus: usize| {
            let mut walk = walk(&profile);
            walk.hosts(scope, &hosts, cpus);
            (walk.versions.contexts_hard_hash, walk.examined)
        };
        let all = version(0) + version(1) + version(2);
        let everything = scope(|_| {});
        assert_eq!(summed(&everything, 1), (all, 15));
        for cpus in [2, 16] {
            assert_eq!(summed(&everything, cpus), (2 * all, 15), "{cpus}");
        }
        // Q is two of the three hosts in scope
        let two_of_three = scope(|s| s.nodes = sp("one|three"));
        assert_eq!(summed(&two_of_three, 1), (all, 10));
        assert_eq!(summed(&two_of_three, 2), (all + version(0) + version(2), 10));
        // Q is one host: no workers, whatever the CPUs
        let one_of_three = scope(|s| s.nodes = sp("two"));
        assert_eq!(summed(&one_of_three, 16), (all, 5));
        // Q is empty
        assert_eq!(summed(&scope(|s| s.nodes = sp("nomatch")), 16), (all, 0));
        // S is two hosts, Q both
        let scoped = scope(|s| s.scope_nodes = sp("one|two"));
        assert_eq!(summed(&scoped, 16), (2 * (version(0) + version(1)), 10));
        assert_eq!(summed(&scoped, 1), (version(0) + version(1), 10));
    }

    /// The stop checks. A client that goes away is seen before a metric is looked at: that metric is not
    /// counted and the walk ends. A request over its time is seen after a metric's method: its result is in, it
    /// is counted, and the walk ends. With workers, both are also tested before each host.
    #[test]
    fn a_walk_stops_for_a_client_gone_and_for_its_time() {
        let profile = Profile::default();
        let hosts = [weights_host_as("guid-1", "one"), weights_host_as("guid-2", "two")];
        let everything = scope(|_| {});
        for cpus in [1, 16] {
            // the client goes away at the third question
            let asked = Cell::new(0);
            let gone = |_: &mut i32| {
                asked.set(asked.get() + 1);
                asked.get() >= 3
            };
            let mut walk = Walk { interrupted: &gone, ..walk(&profile) };
            walk.hosts(&everything, &hosts, cpus);
            // with workers the first question is the first host's
            let kept = if cpus == 1 { 2 } else { 1 };
            assert_eq!((walk.was_interrupted, walk.timed_out, walk.examined), (true, false, kept), "{cpus}");
            assert_eq!(found(&walk), all_of("one")[..kept], "{cpus}");
        }

        // over its time from the start: without workers the first metric is done, then the walk ends
        let ten_seconds_ago = || Instant::now() - Duration::from_secs(10);
        let late = || Walk { received: ten_seconds_ago(), timeout_us: 1_000_000, ..walk(&profile) };
        let mut walk = late();
        walk.hosts(&everything, &hosts, 1);
        assert_eq!((walk.timed_out, walk.was_interrupted, walk.examined), (true, false, 1));
        assert_eq!(found(&walk), ["one:a"]);
        // with workers the time is tested before the first host: nothing is done
        let mut walk = late();
        walk.hosts(&everything, &hosts, 16);
        assert_eq!((walk.timed_out, walk.examined, found(&walk).len()), (true, 0, 0));
    }
}
