mod kubernetes;
mod metrics;

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::serve;
use aya::{include_bytes_aligned, maps::RingBuf, programs::TracePoint, Ebpf};
use aya_log::EbpfLogger;
use kubernetes::KubernetesClient;
use log::{error, info, warn};
use metrics::MetricsCollector;
use oom_watcher_common::{EnrichedOomEvent, OomKillEvent};
use tokio::{signal, task};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    info!("Starting OOM Watcher with Kubernetes and Prometheus integration...");

    // Initialize Kubernetes client
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

    // Initialize Prometheus metrics collector
    let metrics_collector = Arc::new(MetricsCollector::new());

    // Start Prometheus metrics server
    let metrics_port = std::env::var("METRICS_PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);

    let app = metrics_collector.clone().create_server(metrics_port);

    // Bind before spawning so a bind failure fails startup loudly, instead of
    // panicking inside a detached task and leaving the process running blind.
    info!(
        "Starting Prometheus metrics server on port {}",
        metrics_port
    );
    let listener = tokio::net::TcpListener::bind(&format!("0.0.0.0:{}", metrics_port)).await?;

    let mut metrics_server = task::spawn(async move {
        if let Err(e) = serve(listener, app).await {
            error!("Metrics server error: {}", e);
        }
    });

    // Bump the memlock rlimit. This is needed for older kernels that don't use the
    // new memcg based accounting, see https://lwn.net/Articles/837122/
    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };
    if ret != 0 {
        warn!("remove limit on locked memory failed, ret is: {}", ret);
    }

    // Load eBPF program
    #[cfg(feature = "ebpf")]
    let mut bpf = {
        #[cfg(debug_assertions)]
        let bpf = Ebpf::load(include_bytes_aligned!(
            "../../target/bpfel-unknown-none/release/oom-watcher-ebpf"
        ))?;
        #[cfg(not(debug_assertions))]
        let bpf = Ebpf::load(include_bytes_aligned!(concat!(
            env!("OUT_DIR"),
            "/oom-watcher-ebpf-object"
        )))?;
        bpf
    };

    #[cfg(feature = "ebpf")]
    {
        info!("eBPF program loaded successfully");

        if let Err(e) = EbpfLogger::init(&mut bpf) {
            warn!("failed to initialize eBPF logger: {}", e);
        }

        let program: &mut TracePoint = bpf
            .program_mut("mark_victim")
            .ok_or("Could not find eBPF program 'mark_victim'")?
            .try_into()?;

        info!("Loading eBPF program (tracepoint 'oom:mark_victim')...");
        if let Err(e) = program.load() {
            error!("Failed to load eBPF program: {}", e);
            return Err(e.into());
        }

        info!("Attaching to tracepoint oom:mark_victim...");
        if let Err(e) = program.attach("oom", "mark_victim") {
            error!("Failed to attach to tracepoint oom:mark_victim: {}", e);
            error!("This might mean:");
            error!("  1. The tracepoint 'oom:mark_victim' isn't available on this kernel");
            error!("  2. Insufficient permissions (try running as root)");
            return Err(e.into());
        }
        info!("Successfully attached to tracepoint oom:mark_victim");
    }

    #[cfg(not(feature = "ebpf"))]
    {
        info!("eBPF functionality disabled - running in test mode");
    }

    info!("🔍 OOM Watcher is now active and monitoring for OOM events...");
    info!(
        "📊 Prometheus metrics available at http://0.0.0.0:{}/metrics",
        metrics_port
    );
    info!("⏹️  Press Ctrl-C to stop monitoring");

    // Main event processing loop
    let mut event_processor = task::spawn(async move {
        #[cfg(feature = "ebpf")]
        {
            let events_map = match bpf.map_mut("EVENTS") {
                Some(map) => map,
                None => {
                    error!("Could not find eBPF map 'EVENTS'; event processor stopping");
                    return;
                }
            };
            let mut ring_buf = match RingBuf::try_from(events_map) {
                Ok(ring_buf) => ring_buf,
                Err(e) => {
                    error!("Failed to open 'EVENTS' ring buffer: {}", e);
                    return;
                }
            };
            loop {
                while let Some(item) = ring_buf.next() {
                    let data: &[u8] = &item;
                    if data.len() >= core::mem::size_of::<OomKillEvent>() {
                        let ptr = data.as_ptr() as *const OomKillEvent;
                        let raw_event = unsafe { ptr.read_unaligned() };

                        let timestamp = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();

                        // Enrich event with Kubernetes metadata
                        let enriched_event = if let Some(ref client) = k8s_client {
                            match client.get_container_info(raw_event.pid).await {
                                Ok(Some((namespace, pod_name, container_name, container_id))) => {
                                    EnrichedOomEvent {
                                        raw_event,
                                        node_name: Some(client.node_name().to_string()),
                                        namespace: Some(namespace),
                                        pod_name: Some(pod_name),
                                        container_name: Some(container_name),
                                        container_id: Some(container_id),
                                        timestamp,
                                    }
                                }
                                Ok(None) => {
                                    warn!(
                                        "Could not find Kubernetes info for PID {}",
                                        raw_event.pid
                                    );
                                    EnrichedOomEvent {
                                        raw_event,
                                        node_name: k8s_client
                                            .as_ref()
                                            .map(|c| c.node_name().to_string()),
                                        namespace: None,
                                        pod_name: None,
                                        container_name: None,
                                        container_id: None,
                                        timestamp,
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        "Error getting Kubernetes info for PID {}: {}",
                                        raw_event.pid, e
                                    );
                                    EnrichedOomEvent {
                                        raw_event,
                                        node_name: Some(client.node_name().to_string()),
                                        namespace: None,
                                        pod_name: None,
                                        container_name: None,
                                        container_id: None,
                                        timestamp,
                                    }
                                }
                            }
                        } else {
                            EnrichedOomEvent {
                                raw_event,
                                node_name: None,
                                namespace: None,
                                pod_name: None,
                                container_name: None,
                                container_id: None,
                                timestamp,
                            }
                        };

                        // Record metrics
                        metrics_collector.record_oom_event(&enriched_event);

                        // Log the event
                        let comm_str = std::str::from_utf8(&raw_event.comm)
                            .unwrap_or("?")
                            .trim_end_matches('\0');

                        info!("🚨 OOM EVENT DETECTED:");
                        info!("   Process: {} (PID: {})", comm_str, raw_event.pid);
                        if let Some(ref ns) = enriched_event.namespace {
                            info!(
                                "   Kubernetes: {}/{}/{}",
                                ns,
                                enriched_event.pod_name.as_deref().unwrap_or("unknown"),
                                enriched_event
                                    .container_name
                                    .as_deref()
                                    .unwrap_or("unknown")
                            );
                        }
                        info!(
                            "   Memory: total-vm={}kB anon-rss={}kB file-rss={}kB shmem-rss={}kB",
                            raw_event.total_vm,
                            raw_event.anon_rss,
                            raw_event.file_rss,
                            raw_event.shmem_rss
                        );
                        info!(
                            "   User: UID={} pgtables={}kB oom_score_adj={}",
                            raw_event.uid, raw_event.pgtables, raw_event.oom_score_adj
                        );
                    } else {
                        warn!("Received short event: {} bytes", data.len());
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        #[cfg(not(feature = "ebpf"))]
        {
            // In non-eBPF mode, just sleep and wait for shutdown
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    // Run until shutdown is requested or a worker task exits unexpectedly. If a
    // worker dies, return an error so the process exits non-zero and the
    // DaemonSet restarts the pod, rather than staying up but no longer watching.
    let outcome: Result<(), Box<dyn std::error::Error>> = tokio::select! {
        res = signal::ctrl_c() => {
            res?;
            info!("Shutting down OOM Watcher...");
            Ok(())
        }
        res = &mut event_processor => {
            error!("Event processor task exited unexpectedly: {:?}", res);
            Err("event processor task exited".into())
        }
        res = &mut metrics_server => {
            error!("Metrics server task exited unexpectedly: {:?}", res);
            Err("metrics server task exited".into())
        }
    };

    event_processor.abort();
    metrics_server.abort();

    outcome
}
