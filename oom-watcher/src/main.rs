use aya::maps::RingBuf;
use aya::programs::TracePoint;
use aya::util::online_cpus;
use aya::{include_bytes_aligned, Ebpf};
use aya_log::EbpfLogger;
use bytes::BytesMut;
use log::{info, warn, error};
use oom_watcher_common::OomKillEvent;
use tokio::{signal, task};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    info!("Starting OOM Watcher...");

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

    // This will include your eBPF object file as raw bytes at compile time and load it at runtime.
    // This approach is recommended for most real-world use cases. If you would like to specify the
    // eBPF program at runtime rather than at compile time, you can reach for `Ebpf::load_file` instead.
    #[cfg(debug_assertions)]
    let mut bpf = Ebpf::load(include_bytes_aligned!(
        "../../target/bpfel-unknown-none/debug/oom-watcher-ebpf"
    ))?;
    #[cfg(not(debug_assertions))]
    let mut bpf = Ebpf::load(include_bytes_aligned!(concat!(
        env!("OUT_DIR"),
        "/oom-watcher-ebpf-object"
    )))?;

    info!("eBPF program loaded successfully");

    if let Err(e) = EbpfLogger::init(&mut bpf) {
        // This can happen if you remove all log statements from your eBPF program.
        warn!("failed to initialize eBPF logger: {}", e);
    }
    
    // List available programs for debugging
    info!("Available eBPF programs:");
    for (name, _) in bpf.programs() {
        info!("  - {}", name);
    }
    
    // List available maps for debugging
    info!("Available eBPF maps:");
    for (name, _) in bpf.maps() {
        info!("  - {}", name);
    }
    
    let program: &mut TracePoint = bpf.program_mut("mark_victim")
        .ok_or("Could not find eBPF program 'mark_victim'")?
        .try_into()?;
    
    info!("Loading eBPF program (tracepoint 'oom:mark_victim')...");
    if let Err(e) = program.load() {
        error!("Failed to load eBPF program: {}", e);
        return Err(e.into());
    }
    info!("eBPF program loaded successfully");
    
    info!("Attaching to tracepoint oom:mark_victim...");
    if let Err(e) = program.attach("oom", "mark_victim") {
        error!("Failed to attach to tracepoint oom:mark_victim: {}", e);
        error!("This might mean:");
        error!("  1. The tracepoint 'oom:mark_victim' isn't available on this kernel");
        error!("  2. Insufficient permissions (try running as root)");
        return Err(e.into());
    }
    info!("Successfully attached to tracepoint oom:mark_victim");

    let mut ring_buf = RingBuf::try_from(
        bpf.map_mut("EVENTS").ok_or("Could not find map 'EVENTS'")?,
    )?;

    info!("🔍 OOM Watcher is now active and monitoring via ring buffer...");

    loop {
        while let Some(item) = ring_buf.next() {
            let data: &[u8] = &item;
            if data.len() >= core::mem::size_of::<OomKillEvent>() {
                let ptr = data.as_ptr() as *const OomKillEvent;
                let event = unsafe { ptr.read_unaligned() };
                
                // Convert comm bytes to string, handling null termination
                let comm_str = std::str::from_utf8(&event.comm)
                    .unwrap_or("?")
                    .trim_end_matches('\0');
                
                info!("🚨 OOM EVENT DETECTED:");
                info!("   Process: {} (PID: {})", comm_str, event.pid);
                info!("   Memory usage: total-vm={}kB anon-rss={}kB file-rss={}kB shmem-rss={}kB", 
                      event.total_vm, event.anon_rss, event.file_rss, event.shmem_rss);
                info!("   User: UID={} pgtables={}kB oom_score_adj={}", 
                      event.uid, event.pgtables, event.oom_score_adj);
            } else {
                warn!("Received short event: {} bytes", data.len());
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    info!("🔍 OOM Watcher is now active and monitoring for OOM events...");
    info!("💡 Trigger an OOM condition to test (e.g., run the trigger_oom.py script)");
    info!("⏹️  Press Ctrl-C to stop monitoring");
    
    signal::ctrl_c().await?;
    info!("Shutting down OOM Watcher...");

    Ok(())
}
