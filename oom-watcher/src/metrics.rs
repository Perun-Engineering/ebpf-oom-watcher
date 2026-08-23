use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

use oom_watcher_common::{EnrichedOomEvent, OomKillEvent};
use prometheus::{CounterVec, GaugeVec, Registry, TextEncoder};

use crate::resolve::ResolutionOutcome;

/// The `memory_type` label values of `oom_memory_usage_bytes`, in the order
/// [`memory_values`] returns the figures. Recording and eviction both iterate this, so a
/// kind cannot be added to one and forgotten in the other — [`memory_values`] returns an
/// array of exactly this length, so adding a label here without its figure fails to build.
const MEMORY_TYPES: [&str; 4] = ["total_vm", "anon_rss", "file_rss", "shmem_rss"];

/// The memory figures of `raw`, in kilobytes, ordered to match [`MEMORY_TYPES`].
fn memory_values(raw: &OomKillEvent) -> [u64; MEMORY_TYPES.len()] {
    [raw.total_vm, raw.anon_rss, raw.file_rss, raw.shmem_rss]
}

/// The recording seam: how the watch loop reports what it observed, decoupled from
/// Prometheus. The loop depends on this trait, never on the metrics backend.
///
/// `MetricsCollector` is the Prometheus adapter; tests use a spy as the second adapter.
pub trait MetricsRecorder {
    /// Count a resolution that did not yield a container identity, keyed by reason.
    fn record_resolution_outcome(&self, node: &str, outcome: &ResolutionOutcome);

    /// Record an enriched OOM event: kill counts, memory gauges, and timestamp.
    fn record_oom_event(&self, event: &EnrichedOomEvent);

    /// Record the source's monotonic count of events lost before they reached us.
    /// Called with an absolute total, not a delta.
    fn record_dropped_total(&self, node: &str, total: u64);
}

/// The label set shared by every per-container metric, so one entry tracks the lifetime
/// of all of them at once.
///
/// The fields hold the labels *as recorded*, `unknown` fallbacks included — keying on the
/// pre-fallback `Option`s would leave unresolved events unevictable.
#[derive(Clone, PartialEq, Eq, Hash)]
struct SeriesKey {
    node: String,
    namespace: String,
    pod: String,
    container: String,
}

impl SeriesKey {
    /// The label values in the order the metric families declare them.
    fn labels(&self) -> [&str; 4] {
        [&self.node, &self.namespace, &self.pod, &self.container]
    }
}

/// The Prometheus adapter for the [`MetricsRecorder`] seam. Owns the registry and the
/// metric families; HTTP serving lives in [`crate::http`] so axum does not leak through
/// this interface.
pub struct MetricsCollector {
    registry: Registry,
    oom_kills_total: CounterVec,
    oom_kills_per_node_total: CounterVec,
    oom_memory_usage_bytes: GaugeVec,
    oom_last_timestamp: GaugeVec,
    oom_resolution_failures_total: CounterVec,
    oom_events_dropped_total: CounterVec,
    oom_series_evicted_total: CounterVec,
    /// Last absolute drop total seen from the source, so the counter can be advanced by the
    /// delta. There is exactly one node per process, so a single slot suffices.
    last_dropped_total: AtomicU64,
    /// When each per-container label set was last recorded, in wall-clock seconds. This is
    /// the only record of a series' age — a Prometheus registry keeps no such thing — so
    /// it is what [`Self::evict_stale`] sweeps.
    ///
    /// Poisoning is recovered from rather than propagated at both use sites: a panic
    /// elsewhere cannot leave timestamps logically inconsistent, and taking the watcher
    /// down over a poisoned bookkeeping map would lose real OOM events.
    last_seen: Mutex<HashMap<SeriesKey, u64>>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        let registry = Registry::new();

        let oom_kills_total = CounterVec::new(
            prometheus::Opts::new("oom_kills_total", "Total number of OOM kills observed"),
            &["node", "namespace", "pod", "container"],
        )
        .expect("Failed to create oom_kills_total metric");

        let oom_kills_per_node_total = CounterVec::new(
            prometheus::Opts::new(
                "oom_kills_per_node_total",
                "Total number of OOM kills per node",
            ),
            &["node"],
        )
        .expect("Failed to create oom_kills_per_node_total metric");

        let oom_memory_usage_bytes = GaugeVec::new(
            prometheus::Opts::new(
                "oom_memory_usage_bytes",
                "Memory usage in bytes at the time of OOM kill",
            ),
            &["node", "namespace", "pod", "container", "memory_type"],
        )
        .expect("Failed to create oom_memory_usage_bytes metric");

        let oom_last_timestamp = GaugeVec::new(
            prometheus::Opts::new("oom_last_timestamp", "Timestamp of the last OOM kill event"),
            &["node", "namespace", "pod", "container"],
        )
        .expect("Failed to create oom_last_timestamp metric");

        let oom_resolution_failures_total = CounterVec::new(
            prometheus::Opts::new(
                "oom_resolution_failures_total",
                "OOM events whose PID could not be resolved to a container, by reason",
            ),
            &["node", "reason"],
        )
        .expect("Failed to create oom_resolution_failures_total metric");

        let oom_events_dropped_total = CounterVec::new(
            prometheus::Opts::new(
                "oom_events_dropped_total",
                "OOM events the eBPF probe could not enqueue because the ring buffer was full",
            ),
            &["node"],
        )
        .expect("Failed to create oom_events_dropped_total metric");

        let oom_series_evicted_total = CounterVec::new(
            prometheus::Opts::new(
                "oom_series_evicted_total",
                "Per-container metric series deleted after going stale, bounding cardinality",
            ),
            &["node"],
        )
        .expect("Failed to create oom_series_evicted_total metric");

        registry
            .register(Box::new(oom_kills_total.clone()))
            .expect("Failed to register oom_kills_total");
        registry
            .register(Box::new(oom_kills_per_node_total.clone()))
            .expect("Failed to register oom_kills_per_node_total");
        registry
            .register(Box::new(oom_memory_usage_bytes.clone()))
            .expect("Failed to register oom_memory_usage_bytes");
        registry
            .register(Box::new(oom_last_timestamp.clone()))
            .expect("Failed to register oom_last_timestamp");
        registry
            .register(Box::new(oom_resolution_failures_total.clone()))
            .expect("Failed to register oom_resolution_failures_total");
        registry
            .register(Box::new(oom_events_dropped_total.clone()))
            .expect("Failed to register oom_events_dropped_total");
        registry
            .register(Box::new(oom_series_evicted_total.clone()))
            .expect("Failed to register oom_series_evicted_total");

        Self {
            registry,
            oom_kills_total,
            oom_kills_per_node_total,
            oom_memory_usage_bytes,
            oom_last_timestamp,
            oom_resolution_failures_total,
            oom_events_dropped_total,
            oom_series_evicted_total,
            last_dropped_total: AtomicU64::new(0),
            last_seen: Mutex::new(HashMap::new()),
        }
    }

    /// Render the registry in the Prometheus text exposition format.
    pub fn get_metrics(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        encoder
            .encode_to_string(&metric_families)
            .unwrap_or_default()
    }

    /// Delete every per-container series whose last OOM event is at least `ttl_secs` old,
    /// and return how many label sets were dropped.
    ///
    /// This is what bounds cardinality. A pod that OOM-loops mints a fresh label set on
    /// every restart, and a Prometheus registry never expires a series on its own — without
    /// this sweep the process' series count grows for as long as it runs.
    ///
    /// `ttl_secs` must comfortably exceed the scrape interval: a series deleted before it
    /// is scraped takes its increments with it. Node-scoped families are left alone; their
    /// cardinality is one series per process.
    pub fn evict_stale(&self, now: u64, ttl_secs: u64) -> usize {
        // The guard is deliberately held across the removals below. Releasing it first
        // would let a concurrent `record_oom_event` land between the scan and the delete:
        // it would recreate the series and stamp a fresh last-seen time, then have that
        // series deleted underneath it — losing the increment and leaving a last-seen
        // entry with nothing behind it until the next event. Deadlock is not possible,
        // because `with_label_values` releases the metric's own lock before returning, so
        // no thread ever holds one while waiting for this mutex.
        let mut last_seen = self.last_seen.lock().unwrap_or_else(|e| e.into_inner());

        let mut stale = Vec::new();
        // `saturating_sub` absorbs a backwards wall-clock step (an NTP correction), which
        // then reads as a fresh series and merely delays eviction by one sweep.
        last_seen.retain(|key, &mut seen| {
            let fresh = now.saturating_sub(seen) < ttl_secs;
            if !fresh {
                stale.push(key.clone());
            }
            fresh
        });

        for key in &stale {
            self.remove_series(key);
        }
        stale.len()
    }

    /// Delete every series carrying `key`'s label set, and count the eviction.
    ///
    /// `remove_label_values` errors when the series is absent, which is not a failure
    /// here — it means there was nothing left to delete.
    fn remove_series(&self, key: &SeriesKey) {
        let labels = key.labels();

        let _ = self.oom_kills_total.remove_label_values(&labels);
        let _ = self.oom_last_timestamp.remove_label_values(&labels);

        let [node, namespace, pod, container] = labels;
        for memory_type in MEMORY_TYPES {
            let _ = self.oom_memory_usage_bytes.remove_label_values(&[
                node,
                namespace,
                pod,
                container,
                memory_type,
            ]);
        }

        self.oom_series_evicted_total
            .with_label_values(&[node])
            .inc();
    }
}

impl MetricsRecorder for MetricsCollector {
    /// A `Found` outcome is a no-op — successes are implicit in `oom_kills_total`, so the
    /// failure rate is `failures / kills` in PromQL.
    fn record_resolution_outcome(&self, node: &str, outcome: &ResolutionOutcome) {
        let reason = match outcome {
            ResolutionOutcome::Found(_) => return,
            ResolutionOutcome::NotFound => "not_found",
            ResolutionOutcome::Failed(_) => "error",
        };
        self.oom_resolution_failures_total
            .with_label_values(&[node, reason])
            .inc();
    }

    fn record_oom_event(&self, event: &EnrichedOomEvent) {
        let node = event.node_name.as_deref().unwrap_or("unknown");
        let namespace = event.namespace.as_deref().unwrap_or("unknown");
        let pod = event.pod_name.as_deref().unwrap_or("unknown");
        let container = event.container_name.as_deref().unwrap_or("unknown");

        self.oom_kills_total
            .with_label_values(&[node, namespace, pod, container])
            .inc();

        self.oom_kills_per_node_total
            .with_label_values(&[node])
            .inc();

        // Kernel figures are kilobytes; the gauge is bytes.
        for (memory_type, kilobytes) in MEMORY_TYPES
            .into_iter()
            .zip(memory_values(&event.raw_event))
        {
            self.oom_memory_usage_bytes
                .with_label_values(&[node, namespace, pod, container, memory_type])
                .set((kilobytes * 1024) as f64);
        }

        self.oom_last_timestamp
            .with_label_values(&[node, namespace, pod, container])
            .set(event.timestamp as f64);

        // Touched last, so a series is only tracked once it exists. The event's own
        // timestamp is the age reference, which keeps eviction on the clock injected into
        // the watch loop rather than on a second, independent read of the wall clock.
        self.last_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                SeriesKey {
                    node: node.to_string(),
                    namespace: namespace.to_string(),
                    pod: pod.to_string(),
                    container: container.to_string(),
                },
                event.timestamp,
            );
    }

    /// Advance the counter by the delta since the last reading. `checked_sub` guards the
    /// one case the total can appear to move backwards: a per-CPU sum re-taken across a
    /// CPU hotplug. A counter must never be handed a negative increment.
    fn record_dropped_total(&self, node: &str, total: u64) {
        let previous = self.last_dropped_total.swap(total, Ordering::Relaxed);
        let Some(delta) = total.checked_sub(previous) else {
            return;
        };
        if delta > 0 {
            self.oom_events_dropped_total
                .with_label_values(&[node])
                .inc_by(delta as f64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_failures_by_reason_and_ignores_found() {
        let collector = MetricsCollector::new();

        collector.record_resolution_outcome("node-1", &ResolutionOutcome::NotFound);
        collector.record_resolution_outcome("node-1", &ResolutionOutcome::NotFound);
        collector
            .record_resolution_outcome("node-1", &ResolutionOutcome::Failed(anyhow::anyhow!("x")));
        // Found must not touch the failures counter.
        collector.record_resolution_outcome(
            "node-1",
            &ResolutionOutcome::Found(oom_watcher_common::ContainerIdentity {
                namespace: "p".into(),
                pod_name: "po".into(),
                container_name: "c".into(),
                container_id: "id".into(),
            }),
        );

        let out = collector.get_metrics();
        assert!(
            out.contains("oom_resolution_failures_total{node=\"node-1\",reason=\"not_found\"} 2")
        );
        assert!(out.contains("oom_resolution_failures_total{node=\"node-1\",reason=\"error\"} 1"));
    }

    #[test]
    fn advances_the_drop_counter_by_the_delta_between_readings() {
        let collector = MetricsCollector::new();

        // The source reports an absolute total each time; the counter must not restate it.
        collector.record_dropped_total("node-1", 3);
        collector.record_dropped_total("node-1", 3);
        collector.record_dropped_total("node-1", 10);

        assert!(collector
            .get_metrics()
            .contains("oom_events_dropped_total{node=\"node-1\"} 10"));
    }

    #[test]
    fn ignores_a_drop_total_that_moves_backwards() {
        let collector = MetricsCollector::new();

        collector.record_dropped_total("node-1", 7);
        collector.record_dropped_total("node-1", 2);

        // A counter is never decremented.
        assert!(collector
            .get_metrics()
            .contains("oom_events_dropped_total{node=\"node-1\"} 7"));
    }

    /// An enriched event for `pod` at `timestamp`, resolved to a container identity.
    fn event_at(pod: &str, timestamp: u64) -> EnrichedOomEvent {
        EnrichedOomEvent {
            raw_event: oom_watcher_common::OomKillEvent {
                pid: 1234,
                comm: *b"python\0\0\0\0\0\0\0\0\0\0",
                total_vm: 100,
                anon_rss: 50,
                file_rss: 20,
                shmem_rss: 5,
                uid: 1000,
                pgtables: 8,
                oom_score_adj: 0,
            },
            node_name: Some("node-1".into()),
            namespace: Some("prod".into()),
            pod_name: Some(pod.into()),
            container_name: Some("api".into()),
            container_id: Some("abc123".into()),
            timestamp,
        }
    }

    const T0: u64 = 1_717_000_000;
    const TTL: u64 = 1_800;

    #[test]
    fn evicts_a_series_untouched_for_at_least_the_ttl() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));

        assert_eq!(collector.evict_stale(T0 + TTL, TTL), 1);

        assert!(!collector.get_metrics().contains("api-7d9"));
    }

    #[test]
    fn keeps_a_series_touched_within_the_ttl() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));

        // One second short of the TTL is still fresh — the boundary is inclusive.
        assert_eq!(collector.evict_stale(T0 + TTL - 1, TTL), 0);

        assert!(collector
            .get_metrics()
            .contains("oom_kills_total{container=\"api\",namespace=\"prod\",node=\"node-1\",pod=\"api-7d9\"} 1"));
    }

    #[test]
    fn evicts_only_the_stale_series_and_leaves_fresh_ones() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("crashloop-1", T0));
        collector.record_oom_event(&event_at("steady-1", T0 + TTL));

        assert_eq!(collector.evict_stale(T0 + TTL, TTL), 1);

        let out = collector.get_metrics();
        assert!(!out.contains("crashloop-1"));
        assert!(out.contains("steady-1"));
    }

    #[test]
    fn evicting_removes_every_memory_type_gauge() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));
        assert_eq!(
            collector
                .get_metrics()
                .matches("oom_memory_usage_bytes{")
                .count(),
            MEMORY_TYPES.len()
        );

        collector.evict_stale(T0 + TTL, TTL);

        assert!(!collector.get_metrics().contains("oom_memory_usage_bytes{"));
    }

    #[test]
    fn a_later_touch_extends_the_lifetime_of_a_series() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));
        collector.record_oom_event(&event_at("api-7d9", T0 + TTL));

        // Age is measured from the most recent event, not the first.
        assert_eq!(collector.evict_stale(T0 + TTL, TTL), 0);
    }

    #[test]
    fn forgets_the_evicted_key_so_a_second_sweep_is_a_no_op() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));

        collector.evict_stale(T0 + TTL, TTL);

        assert_eq!(collector.evict_stale(T0 + 2 * TTL, TTL), 0);
    }

    #[test]
    fn a_series_recreated_after_eviction_starts_from_zero() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));
        collector.record_oom_event(&event_at("api-7d9", T0));
        collector.evict_stale(T0 + TTL, TTL);

        collector.record_oom_event(&event_at("api-7d9", T0 + 2 * TTL));

        // A counter reset is the correct reading: it is a different container.
        assert!(collector
            .get_metrics()
            .contains("oom_kills_total{container=\"api\",namespace=\"prod\",node=\"node-1\",pod=\"api-7d9\"} 1"));
    }

    #[test]
    fn counts_evictions_per_node() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));
        collector.record_oom_event(&event_at("api-8f2", T0));

        collector.evict_stale(T0 + TTL, TTL);

        assert!(collector
            .get_metrics()
            .contains("oom_series_evicted_total{node=\"node-1\"} 2"));
    }

    #[test]
    fn does_not_emit_an_eviction_series_until_something_is_evicted() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));

        collector.evict_stale(T0, TTL);

        assert!(!collector
            .get_metrics()
            .contains("oom_series_evicted_total{"));
    }

    #[test]
    fn leaves_node_scoped_series_alone() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));

        collector.evict_stale(T0 + TTL, TTL);

        // Cardinality here is one series per process — there is nothing to bound.
        assert!(collector
            .get_metrics()
            .contains("oom_kills_per_node_total{node=\"node-1\"} 1"));
    }

    #[test]
    fn evicts_the_unknown_label_set_of_an_unresolved_event() {
        let collector = MetricsCollector::new();
        let mut event = event_at("api-7d9", T0);
        event.node_name = None;
        event.namespace = None;
        event.pod_name = None;
        event.container_name = None;
        collector.record_oom_event(&event);
        assert!(collector.get_metrics().contains("pod=\"unknown\""));

        // The key must be the labels as recorded, fallbacks included, or these leak.
        assert_eq!(collector.evict_stale(T0 + TTL, TTL), 1);

        assert!(!collector.get_metrics().contains("pod=\"unknown\""));
    }

    #[test]
    fn does_not_emit_a_drop_series_until_something_is_dropped() {
        let collector = MetricsCollector::new();

        collector.record_dropped_total("node-1", 0);

        assert!(!collector
            .get_metrics()
            .contains("oom_events_dropped_total{"));
    }
}
