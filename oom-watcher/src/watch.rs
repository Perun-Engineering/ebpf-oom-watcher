//! The watch loop: the per-event pipeline that turns OOM kill events into recorded,
//! enriched OOM events. See CONTEXT.md ("Watch loop").
//!
//! [`run`] owns the whole loop, pulling from an [`OomEventSource`], resolving through a
//! [`ContainerResolver`], and reporting to a [`MetricsRecorder`]. It is generic over all
//! three seams plus an injected clock, so the entire pipeline is the test surface: a
//! finite source drives it to completion with no kernel and no Kubernetes.

use std::time::Duration;

use log::{info, warn};
use oom_watcher_common::{EnrichedOomEvent, OomKillEvent};

use crate::{
    enrich::enrich,
    health::HEARTBEAT_INTERVAL_SECONDS,
    metrics::MetricsRecorder,
    resolve::{ContainerResolver, ResolutionOutcome},
};

/// The seam for where OOM kill events reach userspace. `next` yields whole, decoded
/// events; `None` means the stream has ended. A real source never ends, so in production
/// the loop runs until the task is aborted; a finite test source ends and the loop returns.
// Static dispatch only — the loop is generic over a concrete source, never `dyn`.
#[allow(async_fn_in_trait)]
pub trait OomEventSource {
    /// **Must be cancel-safe.** [`run`] races this against the heartbeat ticker in a
    /// `select!`, so the future is dropped whenever a tick wins, and an implementation that
    /// buffers a partly-consumed read in a local would lose it. `RingBufSource` satisfies
    /// this by holding its only await at the readiness edge and draining into a field.
    async fn next(&mut self) -> Option<OomKillEvent>;

    /// Events the source knows it lost before they reached us, as a monotonic total.
    /// `None` when the adapter cannot observe drops — the default, since only the ring
    /// buffer can. Reported after each event so a full buffer shows up as a counter rather
    /// than as an unexplained gap in `oom_kills_total`.
    fn dropped_total(&self) -> Option<u64> {
        None
    }
}

/// Why the loop woke.
enum Wake {
    /// An event to process.
    Event(OomKillEvent),
    /// The source ended. Only a finite test source does this.
    Ended,
    /// The heartbeat ticker fired with no event in sight — the normal case on a node that
    /// is not OOMing.
    Beat,
}

/// Run the watch loop: drain `source`, processing each OOM kill event, until it ends.
///
/// `heartbeat` is stamped after every wakeup, event or tick, and is what the liveness probe
/// reads — see [`crate::health`]. It is deliberately stamped *after* `process_event`, so a
/// loop wedged inside resolution stops reporting rather than reporting on the way in.
pub async fn run<S, R, C, H>(
    mut source: S,
    resolver: Option<impl ContainerResolver>,
    recorder: &R,
    now: C,
    heartbeat: H,
) where
    S: OomEventSource,
    R: MetricsRecorder,
    C: Fn() -> u64,
    H: Fn(u64),
{
    // A quiet node parks on the ring buffer's readiness edge indefinitely, so the loop
    // needs its own reason to wake and say so.
    let mut ticker = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECONDS));
    // The first tick resolves immediately; `mark_started` has already seeded the heartbeat.
    ticker.tick().await;

    loop {
        // The `select!` ends before the body runs, so `source` is free to be borrowed again
        // below — hence collapsing the outcome into `Wake` rather than handling it inline.
        let wake = tokio::select! {
            event = source.next() => event.map_or(Wake::Ended, Wake::Event),
            _ = ticker.tick() => Wake::Beat,
        };

        match wake {
            Wake::Ended => break,
            Wake::Event(raw_event) => {
                process_event(&raw_event, resolver.as_ref(), recorder, now()).await;
                if let Some(total) = source.dropped_total() {
                    recorder.record_dropped_total(node_label(resolver.as_ref()), total);
                }
            }
            Wake::Beat => {}
        }

        heartbeat(now());
    }
}

/// The node label for recorder calls: known iff we have a resolver, matching the
/// enrichment rule and the `unknown` fallback the Prometheus adapter already uses.
fn node_label(resolver: Option<&impl ContainerResolver>) -> &str {
    resolver.map_or("unknown", ContainerResolver::node_name)
}

/// Process a single OOM kill event: run resolution (recording the outcome), collapse to an
/// identity, enrich, then record the enriched event. The node name is known iff a resolver
/// exists — the single source of the enrichment iff-rule.
async fn process_event<R: MetricsRecorder>(
    raw_event: &OomKillEvent,
    resolver: Option<&impl ContainerResolver>,
    recorder: &R,
    timestamp: u64,
) {
    let (node_name, identity) = match resolver {
        Some(client) => {
            let outcome = client.resolve(raw_event.pid).await;
            recorder.record_resolution_outcome(client.node_name(), &outcome);
            match &outcome {
                ResolutionOutcome::NotFound => {
                    warn!("Could not find Kubernetes info for PID {}", raw_event.pid)
                }
                ResolutionOutcome::Failed(e) => warn!(
                    "Error getting Kubernetes info for PID {}: {}",
                    raw_event.pid, e
                ),
                ResolutionOutcome::Found(_) => {}
            }
            (Some(client.node_name().to_string()), outcome.identity())
        }
        None => (None, None),
    };

    let enriched = enrich(*raw_event, node_name.as_deref(), identity, timestamp);
    recorder.record_oom_event(&enriched);
    log_event(raw_event, &enriched);
}

fn log_event(raw_event: &OomKillEvent, enriched: &EnrichedOomEvent) {
    let comm_str = std::str::from_utf8(&raw_event.comm)
        .unwrap_or("?")
        .trim_end_matches('\0');

    info!("🚨 OOM EVENT DETECTED:");
    info!("   Process: {} (PID: {})", comm_str, raw_event.pid);
    if let Some(ref ns) = enriched.namespace {
        info!(
            "   Kubernetes: {}/{}/{}",
            ns,
            enriched.pod_name.as_deref().unwrap_or("unknown"),
            enriched.container_name.as_deref().unwrap_or("unknown")
        );
    }
    info!(
        "   Memory: total-vm={}kB anon-rss={}kB file-rss={}kB shmem-rss={}kB",
        raw_event.total_vm, raw_event.anon_rss, raw_event.file_rss, raw_event.shmem_rss
    );
    info!(
        "   User: UID={} pgtables={}kB oom_score_adj={}",
        raw_event.uid, raw_event.pgtables, raw_event.oom_score_adj
    );
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque};

    use oom_watcher_common::ContainerIdentity;

    use super::*;
    use crate::resolve::{Behavior, FakeResolver};

    /// In-memory event source — the second adapter for [`OomEventSource`], so the seam is
    /// real and the loop is drivable in tests. `drops` is the total the source claims to
    /// have lost, mirroring the ring buffer's monotonic counter.
    struct VecSource {
        events: VecDeque<OomKillEvent>,
        drops: Option<u64>,
    }

    impl OomEventSource for VecSource {
        async fn next(&mut self) -> Option<OomKillEvent> {
            self.events.pop_front()
        }

        fn dropped_total(&self) -> Option<u64> {
            self.drops
        }
    }

    /// A source that never yields, so only the heartbeat ticker can wake the loop.
    struct NeverSource;

    impl OomEventSource for NeverSource {
        async fn next(&mut self) -> Option<OomKillEvent> {
            std::future::pending().await
        }
    }

    fn source(events: impl IntoIterator<Item = OomKillEvent>) -> VecSource {
        VecSource {
            events: events.into_iter().collect(),
            drops: None,
        }
    }

    fn source_reporting_drops(
        events: impl IntoIterator<Item = OomKillEvent>,
        drops: u64,
    ) -> VecSource {
        VecSource {
            events: events.into_iter().collect(),
            drops: Some(drops),
        }
    }

    /// Recording spy — the second adapter for [`MetricsRecorder`]. Captures every call so
    /// tests assert what the loop reported, with no Prometheus involved.
    #[derive(Default)]
    struct SpyRecorder {
        outcomes: RefCell<Vec<(String, &'static str)>>,
        events: RefCell<Vec<EnrichedOomEvent>>,
        drops: RefCell<Vec<(String, u64)>>,
    }

    impl MetricsRecorder for SpyRecorder {
        fn record_resolution_outcome(&self, node: &str, outcome: &ResolutionOutcome) {
            let reason = match outcome {
                ResolutionOutcome::Found(_) => "found",
                ResolutionOutcome::NotFound => "not_found",
                ResolutionOutcome::Failed(_) => "error",
            };
            self.outcomes.borrow_mut().push((node.to_string(), reason));
        }

        fn record_oom_event(&self, event: &EnrichedOomEvent) {
            self.events.borrow_mut().push(event.clone());
        }

        fn record_dropped_total(&self, node: &str, total: u64) {
            self.drops.borrow_mut().push((node.to_string(), total));
        }
    }

    fn raw(pid: u32) -> OomKillEvent {
        OomKillEvent {
            pid,
            comm: *b"target\0\0\0\0\0\0\0\0\0\0",
            total_vm: 100,
            anon_rss: 50,
            file_rss: 10,
            shmem_rss: 5,
            uid: 1000,
            pgtables: 2,
            oom_score_adj: 0,
        }
    }

    fn identity() -> ContainerIdentity {
        ContainerIdentity {
            namespace: "prod".into(),
            pod_name: "api-7d9".into(),
            container_name: "api".into(),
            container_id: "containerd://abc123".into(),
            image_id: "repo@sha256:def456".into(),
        }
    }

    const CLOCK: u64 = 1_717_000_000;
    fn clock() -> u64 {
        CLOCK
    }

    #[tokio::test]
    async fn enriches_and_records_a_resolved_event() {
        let spy = SpyRecorder::default();
        let resolver = Some(FakeResolver {
            node: "node-1".into(),
            behavior: Behavior::Found(identity()),
        });

        run(source([raw(1234)]), resolver, &spy, clock, |_| {}).await;

        let events = spy.events.borrow();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.node_name.as_deref(), Some("node-1"));
        assert_eq!(e.namespace.as_deref(), Some("prod"));
        assert_eq!(e.pod_name.as_deref(), Some("api-7d9"));
        assert_eq!(e.container_name.as_deref(), Some("api"));
        assert_eq!(e.raw_event.pid, 1234);
        assert_eq!(e.timestamp, CLOCK);
        // The loop forwards every outcome; the adapter decides to ignore Found.
        assert_eq!(
            *spy.outcomes.borrow(),
            vec![("node-1".to_string(), "found")]
        );
    }

    #[tokio::test]
    async fn keeps_node_but_no_identity_when_not_found() {
        let spy = SpyRecorder::default();
        let resolver = Some(FakeResolver {
            node: "node-1".into(),
            behavior: Behavior::NotFound,
        });

        run(source([raw(1)]), resolver, &spy, clock, |_| {}).await;

        let events = spy.events.borrow();
        assert_eq!(events[0].node_name.as_deref(), Some("node-1"));
        assert_eq!(events[0].namespace, None);
        assert_eq!(events[0].container_id, None);
        assert_eq!(
            *spy.outcomes.borrow(),
            vec![("node-1".to_string(), "not_found")]
        );
    }

    #[tokio::test]
    async fn records_failure_outcome_and_keeps_node() {
        let spy = SpyRecorder::default();
        let resolver = Some(FakeResolver {
            node: "node-1".into(),
            behavior: Behavior::Fail,
        });

        run(source([raw(1)]), resolver, &spy, clock, |_| {}).await;

        assert_eq!(spy.events.borrow()[0].node_name.as_deref(), Some("node-1"));
        assert_eq!(spy.events.borrow()[0].namespace, None);
        assert_eq!(
            *spy.outcomes.borrow(),
            vec![("node-1".to_string(), "error")]
        );
    }

    #[tokio::test]
    async fn standalone_mode_has_no_node_and_records_no_outcome() {
        let spy = SpyRecorder::default();
        let resolver: Option<FakeResolver> = None;

        run(source([raw(1)]), resolver, &spy, clock, |_| {}).await;

        assert_eq!(spy.events.borrow()[0].node_name, None);
        assert!(spy.outcomes.borrow().is_empty());
    }

    #[tokio::test]
    async fn reports_the_sources_drop_total_alongside_each_event() {
        let spy = SpyRecorder::default();
        let resolver = Some(FakeResolver {
            node: "node-1".into(),
            behavior: Behavior::NotFound,
        });

        run(
            source_reporting_drops([raw(1), raw(2)], 4),
            resolver,
            &spy,
            clock,
            |_| {},
        )
        .await;

        assert_eq!(
            *spy.drops.borrow(),
            vec![("node-1".to_string(), 4), ("node-1".to_string(), 4)]
        );
    }

    #[tokio::test]
    async fn reports_no_drops_when_the_source_cannot_observe_them() {
        let spy = SpyRecorder::default();
        let resolver = Some(FakeResolver {
            node: "node-1".into(),
            behavior: Behavior::NotFound,
        });

        run(source([raw(1)]), resolver, &spy, clock, |_| {}).await;

        assert!(spy.drops.borrow().is_empty());
    }

    #[tokio::test]
    async fn labels_drops_unknown_in_standalone_mode() {
        let spy = SpyRecorder::default();
        let resolver: Option<FakeResolver> = None;

        run(
            source_reporting_drops([raw(1)], 2),
            resolver,
            &spy,
            clock,
            |_| {},
        )
        .await;

        assert_eq!(*spy.drops.borrow(), vec![("unknown".to_string(), 2)]);
    }

    #[tokio::test]
    async fn stamps_a_heartbeat_after_every_event() {
        let spy = SpyRecorder::default();
        let resolver = Some(FakeResolver {
            node: "n".into(),
            behavior: Behavior::NotFound,
        });
        let beats = RefCell::new(Vec::new());

        run(
            source([raw(1), raw(2), raw(3)]),
            resolver,
            &spy,
            clock,
            |at| beats.borrow_mut().push(at),
        )
        .await;

        assert_eq!(*beats.borrow(), vec![CLOCK, CLOCK, CLOCK]);
    }

    /// The case the probe exists for: a node that is not OOMing at all. Without the ticker
    /// the loop would park on the source forever and the heartbeat would go stale, failing
    /// liveness on a perfectly healthy watcher.
    #[tokio::test(start_paused = true)]
    async fn stamps_a_heartbeat_while_no_events_arrive() {
        let spy = SpyRecorder::default();
        let resolver: Option<FakeResolver> = None;
        let beats = RefCell::new(Vec::new());

        // Paused time auto-advances while the loop is idle, so this covers three ticks
        // without waiting for them.
        let _ = tokio::time::timeout(
            Duration::from_secs(3 * HEARTBEAT_INTERVAL_SECONDS + 5),
            run(NeverSource, resolver, &spy, clock, |at| {
                beats.borrow_mut().push(at)
            }),
        )
        .await;

        assert_eq!(beats.borrow().len(), 3);
        assert!(spy.events.borrow().is_empty());
    }

    #[tokio::test]
    async fn drains_every_event_in_order() {
        let spy = SpyRecorder::default();
        let resolver = Some(FakeResolver {
            node: "n".into(),
            behavior: Behavior::NotFound,
        });

        run(
            source([raw(1), raw(2), raw(3)]),
            resolver,
            &spy,
            clock,
            |_| {},
        )
        .await;

        let pids: Vec<u32> = spy
            .events
            .borrow()
            .iter()
            .map(|e| e.raw_event.pid)
            .collect();
        assert_eq!(pids, vec![1, 2, 3]);
    }
}
