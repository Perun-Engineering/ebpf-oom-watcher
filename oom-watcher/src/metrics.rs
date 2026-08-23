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

/// How long a deleted pod's series survive the pod itself.
///
/// Same invariant as the eviction TTL, for the same reason: this must comfortably exceed
/// the scrape interval, or a pod that OOMs and is deleted moments later loses the very
/// increment that recorded it. The chart's `serviceMonitor.interval` defaults to 30s, so
/// 120s is 4x headroom while still collapsing a deleted pod's cardinality in minutes
/// rather than in the TTL's half hour.
pub const DELETED_POD_GRACE_SECONDS: u64 = 120;

/// The labels identifying the container a kill is attributed to, in the order
/// [`SeriesKey::labels`] returns them. Carried by both metrics keyed on it —
/// `oom_kills_total` and `oom_last_timestamp` — and kept identical so the two join to
/// each other directly rather than on a subset.
///
/// `container_id` is the kubelet's runtime-prefixed form and `image_id` the digest it
/// resolved, which is what makes these joinable to `kube_pod_container_info` on a key more
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

/// The label set identifying one killed container, so one entry tracks the lifetime of
/// every series describing it.
///
/// The fields hold the labels *as recorded*, `unknown` fallbacks included — keying on the
/// pre-fallback `Option`s would leave unresolved events unevictable.
///
/// `container_id` and `image_id` make this per *restart*, not per container name: a
/// crashlooping pod mints a fresh key each time the runtime replaces the container. That
/// is the point — the ids are what join to `kube_pod_container_info` — but it means
/// `oom_memory_usage_bytes`, which is deliberately not keyed on them, is shared by every
/// key with the same [`Self::memory_labels`] prefix. See [`MetricsCollector::evict_stale`].
#[derive(Clone, PartialEq, Eq, Hash)]
struct SeriesKey {
    node: String,
    namespace: String,
    pod: String,
    container: String,
    container_id: String,
    image_id: String,
}

impl SeriesKey {
    /// The label values in the order the metric families declare them.
    fn labels(&self) -> [&str; 6] {
        [
            &self.node,
            &self.namespace,
            &self.pod,
            &self.container,
            &self.container_id,
            &self.image_id,
        ]
    }

    /// `oom_memory_usage_bytes` carries neither id, so it takes the leading four labels
    /// plus its own `memory_type`.
    fn memory_labels<'a>(&'a self, memory_type: &'a str) -> [&'a str; 5] {
        [
            &self.node,
            &self.namespace,
            &self.pod,
            &self.container,
            memory_type,
        ]
    }

    /// Whether `other` describes the same container name, ignoring which restart it was —
    /// i.e. whether the two share an `oom_memory_usage_bytes` series.
    fn shares_memory_series_with(&self, other: &Self) -> bool {
        self.node == other.node
            && self.namespace == other.namespace
            && self.pod == other.pod
            && self.container == other.container
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
    /// Pods the API server has told us are gone, mapped to the wall-clock second at which
    /// their series may be removed — `deletion time + `[`DELETED_POD_GRACE_SECONDS`].
    ///
    /// This is what makes eviction track pod lifecycle instead of only a timer. It holds a
    /// *due time* rather than deleting on the spot because the P2-1 invariant still binds:
    /// a series removed before it is scraped takes its increments with it, and a Job pod
    /// that OOMs and is deleted seconds later is exactly that case.
    ///
    /// Keyed on `(namespace, pod)` because that is all a pod deletion identifies — the
    /// container names and ids live in the [`SeriesKey`]s it matches.
    deleted_after: Mutex<HashMap<(String, String), u64>>,
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
                "Peak memory usage in bytes observed at OOM kill, per memory type",
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
            deleted_after: Mutex::new(HashMap::new()),
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
    ///
    /// The TTL is the backstop, not the only trigger: a pod the API server has deleted is
    /// swept on its own schedule via [`Self::note_pod_deleted`], so cardinality tracks pod
    /// lifecycle and only falls back to the timer for series nothing has told us about.
    /// The API server says this pod is gone, so its series may go once
    /// [`DELETED_POD_GRACE_SECONDS`] have passed and a final scrape has had its chance.
    ///
    /// Called from the pod cache's watch, which is why it only schedules: removal still
    /// happens in [`Self::evict_stale`], so the `oom_memory_usage_bytes` orphan gate, the
    /// lock discipline and `oom_series_evicted_total` all apply unchanged rather than
    /// being reimplemented on a second deletion path.
    ///
    /// A container restarting does *not* reach here: a crashlooping pod keeps its pod
    /// object, so the case P2-1 and P2-2 were built around is untouched by this.
    pub fn note_pod_deleted(&self, namespace: &str, pod: &str, now: u64) {
        let mut deleted_after = self.deleted_after.lock().unwrap_or_else(|e| e.into_inner());
        deleted_after.insert(
            (namespace.to_string(), pod.to_string()),
            now.saturating_add(DELETED_POD_GRACE_SECONDS),
        );
    }

    pub fn evict_stale(&self, now: u64, ttl_secs: u64) -> usize {
        // Two locks are involved, so the order is fixed and never overlapping:
        // `deleted_after` is taken, drained into a local, and released *before*
        // `last_seen` is taken. Nothing ever holds both, so P2-1's no-cycle argument for
        // the single mutex still stands as written.
        //
        // Draining every due entry — not only those matching a live series — is what
        // bounds this map. The common case by far is a pod deleted without ever OOMing,
        // which matches no key and would otherwise sit here for the life of the process.
        let due: Vec<(String, String)> = {
            let mut deleted_after = self.deleted_after.lock().unwrap_or_else(|e| e.into_inner());
            let mut due = Vec::new();
            deleted_after.retain(|pod, &mut removable_at| {
                let pending = now < removable_at;
                if !pending {
                    due.push(pod.clone());
                }
                pending
            });
            due
        };

        // The guard below is deliberately held across the removals. Releasing it first
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
            let aged_out = now.saturating_sub(seen) >= ttl_secs;
            // A deleted pod's series goes on the pod's schedule rather than the TTL's,
            // however recently it was recorded — the container it describes cannot come
            // back, so there is nothing left for the series to accumulate.
            let pod_gone = due
                .iter()
                .any(|(namespace, pod)| key.namespace == *namespace && key.pod == *pod);
            if aged_out || pod_gone {
                stale.push(key.clone());
                return false;
            }
            true
        });

        // `retain` has already dropped the stale keys, so what remains is exactly the live
        // set — the liveness test below must run against it, not against the map as it was
        // before the scan, or a stale sibling would keep its own memory series alive.
        for key in &stale {
            let memory_series_orphaned = !last_seen
                .keys()
                .any(|live| live.shares_memory_series_with(key));
            self.remove_series(key, memory_series_orphaned);
        }
        stale.len()
    }

    /// Delete every series carrying `key`'s label set, and count the eviction.
    ///
    /// `oom_memory_usage_bytes` is removed only when `memory_series_orphaned` — it is not
    /// keyed on the two ids, so a crashlooping container's restarts all write to one
    /// series and the earliest key going stale must not delete it out from under a live
    /// one. It goes when the last key naming that container does.
    ///
    /// Removing it is also what bounds the peak it holds: each series is the maximum
    /// since it was created, so deleting it is what lets the next kill start from zero.
    ///
    /// `remove_label_values` errors when the series is absent, which is not a failure
    /// here — it means there was nothing left to delete.
    fn remove_series(&self, key: &SeriesKey, memory_series_orphaned: bool) {
        let labels = key.labels();

        let _ = self.oom_kills_total.remove_label_values(&labels);
        let _ = self.oom_last_timestamp.remove_label_values(&labels);

        if memory_series_orphaned {
            for memory_type in MEMORY_TYPES {
                let _ = self
                    .oom_memory_usage_bytes
                    .remove_label_values(&key.memory_labels(memory_type));
            }
        }

        self.oom_series_evicted_total
            .with_label_values(&[key.node.as_str()])
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
        let container_id = event.container_id.as_deref().unwrap_or("unknown");
        let image_id = event.image_id.as_deref().unwrap_or("unknown");
        let per_container = &[node, namespace, pod, container, container_id, image_id];

        self.oom_kills_total.with_label_values(per_container).inc();

        self.oom_kills_per_node_total
            .with_label_values(&[node])
            .inc();

        // Kernel figures are kilobytes; the gauge is bytes.
        //
        // The peak, not the latest. A memcg OOM can kill several processes in one
        // container, and this label set carries no `container_id` to tell them apart, so
        // a plain `set` lets whichever was reaped last win — routinely the container's
        // init, whose `anon_rss=0` then overwrites the process that actually hit the
        // limit. Each `memory_type` peaks independently, so one label set can pair one
        // victim's `anon_rss` with another's `file_rss`: the series answer "how large did
        // this kind get", not "what did one process look like".
        //
        // The window the peak spans is the series' own lifetime. `evict_stale` deletes
        // the series once the container stops being killed, and the next kill starts a
        // fresh maximum from zero.
        //
        // The read-modify-write is safe unsynchronised because this is the only writer:
        // `record_oom_event` is called from the single watch loop task.
        for (memory_type, kilobytes) in MEMORY_TYPES
            .into_iter()
            .zip(memory_values(&event.raw_event))
        {
            let gauge = self.oom_memory_usage_bytes.with_label_values(&[
                node,
                namespace,
                pod,
                container,
                memory_type,
            ]);
            let bytes = (kilobytes * 1024) as f64;
            if bytes > gauge.get() {
                gauge.set(bytes);
            }
        }

        self.oom_last_timestamp
            .with_label_values(per_container)
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
                    container_id: container_id.to_string(),
                    image_id: image_id.to_string(),
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

    /// An enriched event for `pod` at `timestamp`, resolved to a container identity.
    fn event_at(pod: &str, timestamp: u64) -> EnrichedOomEvent {
        event_for(pod, "containerd://abc123", timestamp)
    }

    /// As [`event_at`], but naming the container instance — two ids under one pod name are
    /// the same container restarted, which is what a crashloop looks like here.
    fn event_for(pod: &str, container_id: &str, timestamp: u64) -> EnrichedOomEvent {
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
            container_id: Some(container_id.into()),
            image_id: Some("repo@sha256:def456".into()),
            timestamp,
        }
    }

    /// Labels as Prometheus renders them: sorted, so the two ids land between `container`
    /// and `namespace`.
    const RESOLVED_LABELS: &str = concat!(
        r#"{container="api",container_id="containerd://abc123","#,
        r#"image_id="repo@sha256:def456",namespace="prod",node="node-1",pod="api-7d9"}"#
    );

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
            .contains(&format!("oom_kills_total{RESOLVED_LABELS} 1")));
    }

    #[test]
    fn a_deleted_pods_series_goes_on_the_pods_schedule_not_the_ttls() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));
        collector.note_pod_deleted("prod", "api-7d9", T0);

        // Nowhere near the TTL, but the pod is gone and the grace has passed.
        assert_eq!(
            collector.evict_stale(T0 + DELETED_POD_GRACE_SECONDS, TTL),
            1
        );

        assert!(!collector.get_metrics().contains("api-7d9"));
    }

    /// The whole reason deletion schedules rather than deletes: a series removed before it
    /// is scraped takes its increments with it, and a Job pod that OOMs and is deleted
    /// seconds later is exactly that case.
    #[test]
    fn a_deleted_pods_series_survives_until_the_grace_has_passed() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));
        collector.note_pod_deleted("prod", "api-7d9", T0);

        assert_eq!(
            collector.evict_stale(T0 + DELETED_POD_GRACE_SECONDS - 1, TTL),
            0
        );

        assert!(collector
            .get_metrics()
            .contains(&format!("oom_kills_total{RESOLVED_LABELS} 1")));
    }

    #[test]
    fn deleting_a_pod_takes_every_restart_of_its_containers() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_for("api-7d9", "containerd://first", T0));
        collector.record_oom_event(&event_for("api-7d9", "containerd://second", T0));
        collector.note_pod_deleted("prod", "api-7d9", T0);

        // Both restarts share one `oom_memory_usage_bytes` series; the pod going means the
        // last key naming that container goes too, so the gauge is orphaned and removed.
        assert_eq!(
            collector.evict_stale(T0 + DELETED_POD_GRACE_SECONDS, TTL),
            2
        );

        assert!(!collector.get_metrics().contains("oom_memory_usage_bytes{"));
    }

    #[test]
    fn a_deleted_pod_leaves_another_pods_series_alone() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));
        collector.record_oom_event(&event_at("worker-1", T0));
        collector.note_pod_deleted("prod", "api-7d9", T0);

        assert_eq!(
            collector.evict_stale(T0 + DELETED_POD_GRACE_SECONDS, TTL),
            1
        );

        assert!(collector.get_metrics().contains("worker-1"));
    }

    /// The overwhelmingly common case — a pod deleted having never OOMed — matches no
    /// series at all. Its bookkeeping entry still has to go, or the map grows for the life
    /// of the process on any cluster that churns pods.
    #[test]
    fn forgets_a_deleted_pod_that_never_oomed() {
        let collector = MetricsCollector::new();
        collector.note_pod_deleted("prod", "never-oomed", T0);

        assert_eq!(
            collector.evict_stale(T0 + DELETED_POD_GRACE_SECONDS, TTL),
            0
        );

        assert!(collector.deleted_after.lock().unwrap().is_empty());
    }

    #[test]
    fn keeps_a_deleted_pods_bookkeeping_until_it_is_due() {
        let collector = MetricsCollector::new();
        collector.note_pod_deleted("prod", "never-oomed", T0);

        assert_eq!(
            collector.evict_stale(T0 + DELETED_POD_GRACE_SECONDS - 1, TTL),
            0
        );

        assert_eq!(collector.deleted_after.lock().unwrap().len(), 1);
    }

    /// Two pods can share a name across namespaces; the deletion of one must not sweep the
    /// other.
    #[test]
    fn distinguishes_pods_of_the_same_name_in_different_namespaces() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_at("api-7d9", T0));
        collector.note_pod_deleted("staging", "api-7d9", T0);

        assert_eq!(
            collector.evict_stale(T0 + DELETED_POD_GRACE_SECONDS, TTL),
            0
        );

        assert!(collector.get_metrics().contains("api-7d9"));
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
            .contains(&format!("oom_kills_total{RESOLVED_LABELS} 1")));
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
    fn labels_oom_kills_total_with_the_container_and_image_ids() {
        let collector = MetricsCollector::new();

        collector.record_oom_event(&event_at("api-7d9", T0));

        assert!(collector
            .get_metrics()
            .contains(&format!("oom_kills_total{RESOLVED_LABELS} 1")));
    }

    #[test]
    fn labels_oom_last_timestamp_with_the_container_and_image_ids() {
        // The two per-container metrics carry the same label set, so they join to each
        // other without a `group_left` on a subset.
        let collector = MetricsCollector::new();

        collector.record_oom_event(&event_at("api-7d9", T0));

        assert!(collector
            .get_metrics()
            .contains(&format!("oom_last_timestamp{RESOLVED_LABELS}")));
    }

    #[test]
    fn leaves_the_memory_gauge_label_set_unchanged() {
        // Already five labels, and memory at kill time is a property of the container, not
        // of the image — the ids would multiply series here for no query anyone runs.
        let collector = MetricsCollector::new();

        collector.record_oom_event(&event_at("api-7d9", T0));

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
        let mut event = event_at("api-7d9", T0);
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
    fn keeps_the_memory_gauge_while_another_restart_of_the_container_is_live() {
        // A crashlooping pod keeps its name and gets a new container id per restart, so
        // each restart is its own key — but they all share one `oom_memory_usage_bytes`
        // series, which is not keyed on the ids. Evicting the first restart must not take
        // that series away from the second.
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_for("api-7d9", "containerd://restart-1", T0));
        collector.record_oom_event(&event_for("api-7d9", "containerd://restart-2", T0 + TTL));

        assert_eq!(collector.evict_stale(T0 + TTL, TTL), 1);

        let out = collector.get_metrics();
        assert!(!out.contains("containerd://restart-1"));
        assert!(out.contains("containerd://restart-2"));
        assert_eq!(
            out.matches("oom_memory_usage_bytes{").count(),
            MEMORY_TYPES.len()
        );
    }

    #[test]
    fn removes_the_memory_gauge_once_the_last_restart_goes_stale() {
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_for("api-7d9", "containerd://restart-1", T0));
        collector.record_oom_event(&event_for("api-7d9", "containerd://restart-2", T0 + TTL));
        collector.evict_stale(T0 + TTL, TTL);

        // Now nothing names that container any more, so the shared series goes too.
        assert_eq!(collector.evict_stale(T0 + 2 * TTL, TTL), 1);

        assert!(!collector.get_metrics().contains("oom_memory_usage_bytes{"));
    }

    #[test]
    fn does_not_emit_a_drop_series_until_something_is_dropped() {
        let collector = MetricsCollector::new();

        collector.record_dropped_total("node-1", 0);

        assert!(!collector
            .get_metrics()
            .contains("oom_events_dropped_total{"));
    }

    /// As [`event_at`], but with the four kernel memory figures in kilobytes, ordered to
    /// match [`MEMORY_TYPES`] so a test reads in the same order the gauges do.
    fn event_using(
        pod: &str,
        timestamp: u64,
        kilobytes: [u64; MEMORY_TYPES.len()],
    ) -> EnrichedOomEvent {
        let mut event = event_at(pod, timestamp);
        let [total_vm, anon_rss, file_rss, shmem_rss] = kilobytes;
        event.raw_event.total_vm = total_vm;
        event.raw_event.anon_rss = anon_rss;
        event.raw_event.file_rss = file_rss;
        event.raw_event.shmem_rss = shmem_rss;
        event
    }

    /// The bytes the registry currently renders for one `memory_type` of the shared gauge,
    /// so assertions compare numbers rather than formatted text. Panics if the series is
    /// absent — its absence is asserted with `contains` where that is the point.
    fn memory_gauge(collector: &MetricsCollector, memory_type: &str) -> f64 {
        let prefix = format!(
            concat!(
                r#"oom_memory_usage_bytes{{container="api",memory_type="{}","#,
                r#"namespace="prod",node="node-1",pod="api-7d9"}} "#
            ),
            memory_type
        );
        collector
            .get_metrics()
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("no {memory_type} series"))
            .trim()
            .parse()
            .expect("gauge value is a number")
    }

    const KIB: f64 = 1024.0;

    #[test]
    fn a_smaller_later_kill_does_not_overwrite_the_peak() {
        // The cascading-kill case this metric got wrong: one memcg OOM kills the process
        // that hit the limit and then the container's init, and both land on this label
        // set because it carries no `container_id`. Under `set`, init's `anon_rss=0` won
        // and an alert read 0 bytes for a 64MB OOM.
        let collector = MetricsCollector::new();

        collector.record_oom_event(&event_using("api-7d9", T0, [70_000, 64_260, 0, 0]));
        collector.record_oom_event(&event_using("api-7d9", T0 + 1, [4_000, 0, 828, 0]));

        assert_eq!(memory_gauge(&collector, "anon_rss"), 64_260.0 * KIB);
        assert_eq!(memory_gauge(&collector, "total_vm"), 70_000.0 * KIB);
    }

    #[test]
    fn a_larger_later_kill_raises_the_peak() {
        // The same rule in the other order — the peak must still track upwards, or it
        // would only ever report whichever kill happened to arrive first.
        let collector = MetricsCollector::new();

        collector.record_oom_event(&event_using("api-7d9", T0, [4_000, 0, 828, 0]));
        collector.record_oom_event(&event_using("api-7d9", T0 + 1, [70_000, 64_260, 0, 0]));

        assert_eq!(memory_gauge(&collector, "anon_rss"), 64_260.0 * KIB);
    }

    #[test]
    fn each_memory_type_peaks_independently() {
        // The consequence of taking the maximum per type: one label set can hold one
        // victim's `anon_rss` beside another's `file_rss`. Each series answers "how large
        // did this kind get", not "what did one process look like".
        let collector = MetricsCollector::new();

        collector.record_oom_event(&event_using("api-7d9", T0, [0, 64_260, 0, 0]));
        collector.record_oom_event(&event_using("api-7d9", T0 + 1, [0, 0, 828, 0]));

        assert_eq!(memory_gauge(&collector, "anon_rss"), 64_260.0 * KIB);
        assert_eq!(memory_gauge(&collector, "file_rss"), 828.0 * KIB);
    }

    #[test]
    fn eviction_resets_the_peak() {
        // The peak spans the series' lifetime, and eviction is what ends it. Without the
        // reset the maximum would be the process' high-water mark for as long as it runs,
        // and a container whose kills got smaller would never say so.
        let collector = MetricsCollector::new();
        collector.record_oom_event(&event_using("api-7d9", T0, [70_000, 64_260, 0, 0]));

        collector.evict_stale(T0 + TTL, TTL);
        collector.record_oom_event(&event_using("api-7d9", T0 + TTL, [4_000, 1_024, 0, 0]));

        assert_eq!(memory_gauge(&collector, "anon_rss"), 1_024.0 * KIB);
    }
}
