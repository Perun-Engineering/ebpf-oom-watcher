mod enrich;
mod http;
mod kubernetes;
mod metrics;
mod resolve;
mod source;
mod watch;

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::anyhow;
use axum::serve;
use kubernetes::KubernetesClient;
use log::{error, info, warn};
use metrics::MetricsCollector;
use resolve::ContainerResolver;
#[cfg(not(feature = "ebpf"))]
use source::ParkSource;
#[cfg(feature = "ebpf")]
use source::RingBufSource;
use tokio::{signal, task};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    info!("Starting OOM Watcher with Kubernetes and Prometheus integration...");

    // Resolver for the watch loop: Some iff in-cluster. A failure drops us to standalone
    // mode (no node, no container identity) rather than aborting startup.
    let k8s_client = match KubernetesClient::new().await {
        Ok(client) => {
            info!(
                "Successfully connected to Kubernetes API on node: {}",
                client.node_name()
            );
            Some(client)
        }
        Err(e) => {
            warn!(
                "Failed to connect to Kubernetes API: {}. Running in standalone mode.",
                e
            );
            None
        }
    };

    // Metrics recorder + its HTTP surface.
    let metrics_collector = Arc::new(MetricsCollector::new());
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
    let app = http::router(metrics_collector.clone());
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
    let mut event_processor = task::spawn(async move {
        watch::run(source, k8s_client, recorder.as_ref(), wall_clock_secs).await;
    });

    // Run until shutdown is requested or a worker task exits unexpectedly. If a worker
    // dies, return an error so the process exits non-zero and the DaemonSet restarts the
    // pod, rather than staying up but no longer watching.
    let outcome: anyhow::Result<()> = tokio::select! {
        res = signal::ctrl_c() => {
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
    };

    event_processor.abort();
    metrics_server.abort();

    outcome
}

/// Wall-clock seconds since the Unix epoch — the clock injected into the watch loop.
fn wall_clock_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
