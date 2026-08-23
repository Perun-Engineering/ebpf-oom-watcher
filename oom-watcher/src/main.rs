mod enrich;
mod health;
mod http;
mod kubernetes;
mod metrics;
mod resolve;
mod source;
mod tracepoint;
mod watch;

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::anyhow;
use axum::serve;
use health::Health;
use kubernetes::KubernetesClient;
use log::{error, info, warn};
use metrics::MetricsCollector;
use resolve::ContainerResolver;
#[cfg(not(feature = "ebpf"))]
use source::ParkSource;
#[cfg(feature = "ebpf")]
use source::RingBufSource;
use tokio::{signal, task};

/// How long a per-container series survives its last OOM event before the sweep deletes
/// it. Must stay well clear of the scrape interval — the chart's `serviceMonitor.interval`
/// defaults to 30s, so this leaves ~60x headroom.
const DEFAULT_SERIES_TTL_SECONDS: u64 = 1800;

/// How often stale series are swept. Eviction is therefore late by up to this much, which
/// is why the sweep is far cheaper than the TTL is long.
const DEFAULT_SERIES_SWEEP_INTERVAL_SECONDS: u64 = 300;

/// How long the watch loop waits for the pod cache's first list before draining events
/// anyway. It bounds how long a kill sits in the ring buffer at startup, not how long the
/// process takes to serve `/metrics`. Ten seconds is generous for one node-scoped list and
/// well inside the ring's ~3k-event headroom.
const CACHE_SYNC_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    info!("Starting OOM Watcher with Kubernetes and Prometheus integration...");

    // Metrics recorder + its HTTP surface. Built before the Kubernetes client because the
    // pod cache reports deletions into it.
    let metrics_collector = Arc::new(MetricsCollector::new());
    // Liveness state, shared between the watch loop (which stamps it) and `/healthz`
    // (which reads it). See `health` for what a stale heartbeat does and does not prove.
    let health = Arc::new(Health::new());

    // Resolver for the watch loop: Some iff in-cluster. A failure drops us to standalone
    // mode (no node, no container identity) rather than aborting startup.
    //
    // The client comes with the task feeding its pod cache. That task is what keeps
    // resolution off the API server, so it is supervised like every other worker below —
    // a cache nobody is feeding still answers, just with stale pods. It also reports pod
    // deletions, which is what lets eviction follow pod lifecycle instead of only a timer.
    let deletions = metrics_collector.clone();
    let (k8s_client, mut pod_cache) = match KubernetesClient::new(move |namespace, pod| {
        deletions.note_pod_deleted(namespace, pod, wall_clock_secs())
    })
    .await
    {
        Ok((client, cache_task)) => {
            info!(
                "Successfully connected to Kubernetes API on node: {}",
                client.node_name()
            );
            (Some(client), Some(cache_task))
        }
        Err(e) => {
            warn!(
                "Failed to connect to Kubernetes API: {}. Running in standalone mode.",
                e
            );
            // Standalone mode has no pods to mirror, so there is no cache task and nothing
            // to supervise. Both are `None` together — the client is what owns the cache.
            (None, None)
        }
    };

    let metrics_port = std::env::var("METRICS_PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);

    // Bind before spawning so a bind failure fails startup loudly, instead of panicking
    // inside a detached task and leaving the process running blind.
    info!(
        "Starting Prometheus metrics server on port {}",
        metrics_port
    );
    let listener = tokio::net::TcpListener::bind(&format!("0.0.0.0:{}", metrics_port)).await?;
    let app = http::router(
        metrics_collector.clone(),
        health.clone(),
        Arc::new(wall_clock_secs),
    );
    let mut metrics_server = task::spawn(async move {
        if let Err(e) = serve(listener, app).await {
            error!("Metrics server error: {}", e);
        }
    });

    // Event source: the eBPF probe in-cluster, a parking source for non-eBPF builds. All
    // aya/ring-buffer handling lives behind the OomEventSource seam.
    #[cfg(feature = "ebpf")]
    let source = RingBufSource::new()?;
    #[cfg(not(feature = "ebpf"))]
    let source = ParkSource;

    info!("🔍 OOM Watcher is now active and monitoring for OOM events...");
    info!(
        "📊 Prometheus metrics available at http://0.0.0.0:{}/metrics",
        metrics_port
    );
    info!("⏹️  Press Ctrl-C to stop monitoring");

    // The watch loop owns the source and resolver and borrows the recorder for the life of
    // the task. It loops forever in production; the select! below supervises and aborts it.
    let recorder = metrics_collector.clone();
    let loop_health = health.clone();
    let mut event_processor = task::spawn(async move {
        // Let the pod cache list before we start resolving against it. The probe is
        // already attached, so a kill during this window is held in the ring buffer and
        // resolves correctly once the list lands — where draining immediately would
        // report it against an empty cache. Waiting here rather than during startup is
        // what keeps it off the critical path: the HTTP surface is bound above, and this
        // runs inside the supervised task, so a SIGTERM arriving mid-wait is still
        // handled. `/healthz` reports `starting` for the duration.
        if let Some(client) = &k8s_client {
            client.wait_until_synced(CACHE_SYNC_TIMEOUT).await;
        }
        // Everything the loop needs is up, so liveness starts being asserted here rather
        // than at bind time — `/healthz` answers 503 until this point.
        loop_health.mark_started(wall_clock_secs());
        watch::run(
            source,
            k8s_client,
            recorder.as_ref(),
            wall_clock_secs,
            |at| loop_health.beat(at),
        )
        .await;
    });

    // Cardinality sweep. It has to be its own task rather than a step in the watch loop:
    // series go stale precisely when a pod stops OOMing, and the loop is then parked on
    // epoll with nothing to drive it.
    let series_ttl = env_seconds("SERIES_TTL_SECONDS", DEFAULT_SERIES_TTL_SECONDS);
    let sweep_interval = env_seconds(
        "SERIES_SWEEP_INTERVAL_SECONDS",
        DEFAULT_SERIES_SWEEP_INTERVAL_SECONDS,
    );
    info!(
        "Evicting per-container series after {}s, swept every {}s",
        series_ttl, sweep_interval
    );
    let sweeper = metrics_collector.clone();
    let mut series_sweeper = task::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(sweep_interval));
        // The first tick resolves immediately; nothing is stale at startup.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let evicted = sweeper.evict_stale(wall_clock_secs(), series_ttl);
            if evicted > 0 {
                info!("Evicted {} stale metric series", evicted);
            }
        }
    });

    // Run until shutdown is requested or a worker task exits unexpectedly. If a worker
    // dies, return an error so the process exits non-zero and the DaemonSet restarts the
    // pod, rather than staying up but no longer watching.
    //
    // Read once, before the pod cache arm borrows the handle it guards.
    let watching_pods = pod_cache.is_some();
    let outcome: anyhow::Result<()> = tokio::select! {
        res = shutdown_signal() => {
            res?;
            info!("Shutting down OOM Watcher...");
            Ok(())
        }
        res = &mut event_processor => {
            error!("Event processor task exited unexpectedly: {:?}", res);
            Err(anyhow!("event processor task exited"))
        }
        res = &mut metrics_server => {
            error!("Metrics server task exited unexpectedly: {:?}", res);
            Err(anyhow!("metrics server task exited"))
        }
        res = &mut series_sweeper => {
            error!("Series sweeper task exited unexpectedly: {:?}", res);
            Err(anyhow!("series sweeper task exited"))
        }
        // Disabled in standalone mode, where there is no cache task to wait on. The
        // `expect` is unreachable for that reason, and it only runs if the arm is polled.
        res = async { pod_cache.as_mut().expect("enabled only when the cache exists").await },
              if watching_pods => {
            error!("Pod cache task exited unexpectedly: {:?}", res);
            Err(anyhow!("pod cache task exited"))
        }
    };

    event_processor.abort();
    metrics_server.abort();
    series_sweeper.abort();
    if let Some(pod_cache) = &pod_cache {
        pod_cache.abort();
    }

    outcome
}

/// Read a duration in seconds from the environment, falling back to `default` when the
/// variable is unset or unparseable. Clamped to at least one second: `tokio::time::interval`
/// panics on a zero period, and a zero TTL would evict every series before it is scraped.
fn env_seconds(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(default)
        .max(1)
}

/// Resolve when the process is asked to stop.
///
/// SIGTERM is the one that matters in a cluster: it is what the kubelet sends on pod
/// deletion, and ignoring it means every rollout waits out the full
/// `terminationGracePeriodSeconds` before the container is SIGKILLed. SIGINT is kept for
/// running the binary by hand.
async fn shutdown_signal() -> anyhow::Result<()> {
    let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())?;

    tokio::select! {
        res = signal::ctrl_c() => {
            res?;
            info!("Received SIGINT");
        }
        _ = sigterm.recv() => info!("Received SIGTERM"),
    }

    Ok(())
}

/// Wall-clock seconds since the Unix epoch — the clock injected into the watch loop.
fn wall_clock_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
