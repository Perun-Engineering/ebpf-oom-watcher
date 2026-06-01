use oom_watcher_common::EnrichedOomEvent;
use prometheus::{CounterVec, GaugeVec, Registry, TextEncoder};

use crate::resolve::ResolutionOutcome;

/// The recording seam: how the watch loop reports what it observed, decoupled from
/// Prometheus. The loop depends on this trait, never on the metrics backend.
///
/// `MetricsCollector` is the Prometheus adapter; tests use a spy as the second adapter.
pub trait MetricsRecorder {
    /// Count a resolution that did not yield a container identity, keyed by reason.
    fn record_resolution_outcome(&self, node: &str, outcome: &ResolutionOutcome);

    /// Record an enriched OOM event: kill counts, memory gauges, and timestamp.
    fn record_oom_event(&self, event: &EnrichedOomEvent);
}

/// The Prometheus adapter for the [`MetricsRecorder`] seam. Owns the registry and the
/// metric families; HTTP serving lives in [`crate::http`] so axum does not leak through
/// this interface.
#[derive(Clone)]
pub struct MetricsCollector {
    registry: Registry,
    oom_kills_total: CounterVec,
    oom_kills_per_node_total: CounterVec,
    oom_memory_usage_bytes: GaugeVec,
    oom_last_timestamp: GaugeVec,
    oom_resolution_failures_total: CounterVec,
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

        Self {
            registry,
            oom_kills_total,
            oom_kills_per_node_total,
            oom_memory_usage_bytes,
            oom_last_timestamp,
            oom_resolution_failures_total,
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

        // Increment total OOM kills
        self.oom_kills_total
            .with_label_values(&[node, namespace, pod, container])
            .inc();

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
            .with_label_values(&[node, namespace, pod, container])
            .set(event.timestamp as f64);
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
}
