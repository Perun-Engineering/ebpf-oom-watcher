//! Adapters for the OOM event source seam.
//!
//! `RingBufSource` is the in-cluster adapter: it owns the entire eBPF lifecycle — bumping
//! the memlock rlimit, loading the probe, attaching it to `oom:mark_victim`, and draining
//! the ring buffer — and performs the single `unsafe` decode of raw bytes into an
//! `OomKillEvent`. It is the only place `aya` is referenced, which is why `aya`/`aya-log`/
//! `libc` are optional deps gated on the `ebpf` feature. `ParkSource` is the no-op adapter
//! for builds without that feature.

#[cfg(feature = "ebpf")]
mod ebpf_source {
    use std::time::Duration;

    use anyhow::{anyhow, Result};
    use aya::{
        include_bytes_aligned,
        maps::{MapData, RingBuf},
        programs::TracePoint,
        Ebpf,
    };
    use aya_log::EbpfLogger;
    use log::{error, info, warn};
    use oom_watcher_common::OomKillEvent;

    use crate::watch::OomEventSource;

    /// The in-cluster adapter for [`OomEventSource`]. Holds the loaded eBPF program (so the
    /// tracepoint stays attached for the source's lifetime) and owns the ring buffer.
    pub struct RingBufSource {
        // Keeps the program attached; never read directly.
        _bpf: Ebpf,
        ring_buf: RingBuf<MapData>,
    }

    impl RingBufSource {
        /// Bring up the probe end to end: bump the memlock rlimit, load the eBPF object,
        /// attach to `oom:mark_victim`, and take ownership of the `EVENTS` ring buffer.
        pub fn new() -> Result<Self> {
            bump_memlock_rlimit();

            #[cfg(debug_assertions)]
            let mut bpf = Ebpf::load(include_bytes_aligned!(
                "../../target/ebpf-subbuild/bpfel-unknown-none/release/oom-watcher-ebpf"
            ))?;
            #[cfg(not(debug_assertions))]
            let mut bpf = Ebpf::load(include_bytes_aligned!(concat!(
                env!("OUT_DIR"),
                "/oom-watcher-ebpf-object"
            )))?;
            info!("eBPF program loaded successfully");

            if let Err(e) = EbpfLogger::init(&mut bpf) {
                warn!("failed to initialize eBPF logger: {}", e);
            }

            let program: &mut TracePoint = bpf
                .program_mut("mark_victim")
                .ok_or_else(|| anyhow!("Could not find eBPF program 'mark_victim'"))?
                .try_into()?;

            info!("Loading eBPF program (tracepoint 'oom:mark_victim')...");
            program.load()?;

            info!("Attaching to tracepoint oom:mark_victim...");
            if let Err(e) = program.attach("oom", "mark_victim") {
                error!("Failed to attach to tracepoint oom:mark_victim: {}", e);
                error!("This might mean:");
                error!("  1. The tracepoint 'oom:mark_victim' isn't available on this kernel");
                error!("  2. Insufficient permissions (try running as root)");
                return Err(e.into());
            }
            info!("Successfully attached to tracepoint oom:mark_victim");

            let map = bpf
                .take_map("EVENTS")
                .ok_or_else(|| anyhow!("Could not find eBPF map 'EVENTS'"))?;
            let ring_buf = RingBuf::try_from(map)?;

            Ok(Self {
                _bpf: bpf,
                ring_buf,
            })
        }
    }

    impl OomEventSource for RingBufSource {
        async fn next(&mut self) -> Option<OomKillEvent> {
            // Drain everything currently available, then poll. A real probe never ends, so
            // this never returns None; the watch loop is stopped by aborting its task.
            loop {
                while let Some(item) = self.ring_buf.next() {
                    let data: &[u8] = &item;
                    if data.len() >= core::mem::size_of::<OomKillEvent>() {
                        // The eBPF side writes exactly one #[repr(C)] OomKillEvent per entry.
                        let ptr = data.as_ptr() as *const OomKillEvent;
                        let event = unsafe { ptr.read_unaligned() };
                        return Some(event);
                    }
                    warn!("Received short event: {} bytes", data.len());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }

    fn bump_memlock_rlimit() {
        // Needed for older kernels without memcg-based accounting; see
        // https://lwn.net/Articles/837122/
        let rlim = libc::rlimit {
            rlim_cur: libc::RLIM_INFINITY,
            rlim_max: libc::RLIM_INFINITY,
        };
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };
        if ret != 0 {
            warn!("remove limit on locked memory failed, ret is: {}", ret);
        }
    }
}

#[cfg(feature = "ebpf")]
pub use ebpf_source::RingBufSource;

#[cfg(not(feature = "ebpf"))]
mod park_source {
    use std::time::Duration;

    use oom_watcher_common::OomKillEvent;

    use crate::watch::OomEventSource;

    /// A source that never yields, for builds without the `ebpf` feature: the binary still
    /// starts (and serves metrics) but has no probe to read.
    pub struct ParkSource;

    impl OomEventSource for ParkSource {
        async fn next(&mut self) -> Option<OomKillEvent> {
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
    }
}

#[cfg(not(feature = "ebpf"))]
pub use park_source::ParkSource;
