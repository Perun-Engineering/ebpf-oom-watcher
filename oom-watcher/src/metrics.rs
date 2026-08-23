use std::sync::atomic::{AtomicU64, Ordering};

use oom_watcher_common::EnrichedOomEvent;
use prometheus::{CounterVec, GaugeVec, Registry, TextEncoder};

use crate::resolve::ResolutionOutcome;

/// The labels identifying the container a kill is attributed to. Carried by both metrics
/// keyed on it — `oom_kills_total` and `oom_last_timestamp` — and kept identical so the
/// two join to each other directly rather than on a subset.
///
/// `container_id` is the kubelet's runtime-prefixed form and `image_id` the digest it
/// resolved, which is what makes this joinable to `kube_pod_container_info` on a key more
/// stable than pod name — a restarted pod in a Deployment gets a new name, but the ids
/// still describe the thing that was killed. `image_id` is functionally determined by
/// `container_id`, so it annotates these series without multiplying them.
const PER_CONTAINER_LABELS: &[&str] = &[
    "node",
    "namespace",
    "pod",
    "container",
    "container_id",
    "image_id",
];

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
    /// Last absolute drop total seen from the source, so the counter can be advanced by the
    /// delta. There is exactly one node per process, so a single slot suffices.
    last_dropped_total: AtomicU64,
}

impl MetricsCollector {
    pub fn new() -> Self {
        let registry = Registry::new();

        let oom_kills_total = CounterVec::new(
            prometheus::Opts::new("oom_kills_total", "Total number of OOM kills observed"),
            PER_CONTAINER_LABELS,
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
            PER_CONTAINER_LABELS,
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

        Self {
            registry,
            oom_kills_total,
            oom_kills_per_node_total,
            oom_memory_usage_bytes,
            oom_last_timestamp,
            oom_resolution_failures_total,
            oom_events_dropped_total,
            last_dropped_total: AtomicU64::new(0),
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
        let container_id = event.container_id.as_deref().unwrap_or("unknown");
        let image_id = event.image_id.as_deref().unwrap_or("unknown");
        let per_container = &[node, namespace, pod, container, container_id, image_id];

        // Increment total OOM kills
        self.oom_kills_total.with_label_values(per_container).inc();

        // Increment per-node OOM kills
        self.oom_kills_per_node_total
            .with_label_values(&[node])
            .inc();

        // Record memory usage at time of OOM
        let labels = &[node, namespace, pod, container];

        self.oom_memory_usage_bytes
            .with_label_values(&[labels[0], labels[1], labels[2], labels[3], "total_vm"])
            .set((event.raw_event.total_vm * 1024) as f64); // Convert KB to bytes

        self.oom_memory_usage_bytes
            .with_label_values(&[labels[0], labels[1], labels[2], labels[3], "anon_rss"])
            .set((event.raw_event.anon_rss * 1024) as f64);

        self.oom_memory_usage_bytes
            .with_label_values(&[labels[0], labels[1], labels[2], labels[3], "file_rss"])
            .set((event.raw_event.file_rss * 1024) as f64);

        self.oom_memory_usage_bytes
            .with_label_values(&[labels[0], labels[1], labels[2], labels[3], "shmem_rss"])
            .set((event.raw_event.shmem_rss * 1024) as f64);

        // Record timestamp
        self.oom_last_timestamp
            .with_label_values(per_container)
            .set(event.timestamp as f64);
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
                container_id: "containerd://id".into(),
                image_id: "repo@sha256:d".into(),
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

    /// A fully resolved event, as the watch loop hands it to the recorder.
    fn resolved_event() -> EnrichedOomEvent {
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
            pod_name: Some("api-7d9".into()),
            container_name: Some("api".into()),
            container_id: Some("containerd://abc123".into()),
            image_id: Some("repo@sha256:def456".into()),
            timestamp: 1_717_000_000,
        }
    }

    /// Labels as Prometheus renders them: sorted, so container_id and image_id land
    /// between `container` and `namespace`.
    const RESOLVED_LABELS: &str = concat!(
        r#"{container="api",container_id="containerd://abc123","#,
        r#"image_id="repo@sha256:def456",namespace="prod",node="node-1",pod="api-7d9"}"#
    );

    #[test]
    fn labels_oom_kills_total_with_the_container_and_image_ids() {
        let collector = MetricsCollector::new();

        collector.record_oom_event(&resolved_event());

        assert!(collector
            .get_metrics()
            .contains(&format!("oom_kills_total{RESOLVED_LABELS} 1")));
    }

    #[test]
    fn labels_oom_last_timestamp_with_the_container_and_image_ids() {
        // The two per-incident metrics carry the same label set, so they join to each
        // other without a `group_left` on a subset.
        let collector = MetricsCollector::new();

        collector.record_oom_event(&resolved_event());

        assert!(collector
            .get_metrics()
            .contains(&format!("oom_last_timestamp{RESOLVED_LABELS}")));
    }

    #[test]
    fn leaves_the_memory_gauge_label_set_unchanged() {
        // Already five labels, and memory at kill time is a property of the container, not
        // of the image — the ids would multiply series here for no query anyone runs.
        let collector = MetricsCollector::new();

        collector.record_oom_event(&resolved_event());

        let out = collector.get_metrics();
        assert!(out.contains(
            r#"oom_memory_usage_bytes{container="api",memory_type="anon_rss",namespace="prod",node="node-1",pod="api-7d9"}"#
        ));
        assert!(!out
            .lines()
            .any(|l| l.starts_with("oom_memory_usage_bytes") && l.contains("container_id=")));
    }

    #[test]
    fn falls_back_to_unknown_for_both_ids_when_resolution_failed() {
        let collector = MetricsCollector::new();
        let mut event = resolved_event();
        event.namespace = None;
        event.pod_name = None;
        event.container_name = None;
        event.container_id = None;
        event.image_id = None;

        collector.record_oom_event(&event);

        assert!(collector.get_metrics().contains(concat!(
            r#"oom_kills_total{container="unknown",container_id="unknown","#,
            r#"image_id="unknown",namespace="unknown",node="node-1",pod="unknown"} 1"#
        )));
    }

    #[test]
    fn keeps_the_per_node_counter_free_of_container_labels() {
        // One series per node is the point of this metric; it must not gain a dimension.
        let collector = MetricsCollector::new();

        collector.record_oom_event(&resolved_event());

        assert!(collector
            .get_metrics()
            .contains(r#"oom_kills_per_node_total{node="node-1"} 1"#));
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
