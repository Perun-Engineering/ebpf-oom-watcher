use std::{collections::HashMap, sync::Arc};

use axum::{extract::State, http::StatusCode, response::Response, routing::get, Router};
use oom_watcher_common::EnrichedOomEvent;
use prometheus::{Counter, CounterVec, Encoder, Gauge, GaugeVec, Registry, TextEncoder};

#[derive(Clone)]
pub struct MetricsCollector {
    registry: Registry,
    oom_kills_total: CounterVec,
    oom_kills_per_node_total: CounterVec,
    oom_memory_usage_bytes: GaugeVec,
    oom_last_timestamp: GaugeVec,
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

        Self {
            registry,
            oom_kills_total,
            oom_kills_per_node_total,
            oom_memory_usage_bytes,
            oom_last_timestamp,
        }
    }

    pub fn record_oom_event(&self, event: &EnrichedOomEvent) {
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

    pub fn get_metrics(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        encoder
            .encode_to_string(&metric_families)
            .unwrap_or_default()
    }

    pub fn create_server(self, port: u16) -> Router {
        Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(Arc::new(self))
    }
}

async fn metrics_handler(
    State(collector): State<Arc<MetricsCollector>>,
) -> Result<Response<String>, StatusCode> {
    let metrics = collector.get_metrics();
    Ok(Response::builder()
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(metrics)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
}
