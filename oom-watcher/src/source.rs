//! Adapters for the OOM event source seam.
//!
//! `RingBufSource` is the in-cluster adapter: it owns the entire eBPF lifecycle — asserting
//! the kernel's tracepoint layout, bumping the memlock rlimit, loading the probe, attaching
//! it to `oom:mark_victim`, and draining the ring buffer — and performs the single `unsafe`
//! decode of raw bytes into an `OomKillEvent`. It is the only place `aya` is referenced,
//! which is why `aya`/`libc` are optional deps gated on the `ebpf` feature.
//! `ParkSource` is the no-op adapter for builds without that feature.

#[cfg(feature = "ebpf")]
mod ebpf_source {
    use std::collections::VecDeque;

    use anyhow::{anyhow, Context, Result};
    use aya::{
        include_bytes_aligned,
        maps::{MapData, PerCpuArray, RingBuf},
        programs::TracePoint,
        Ebpf,
    };
    use log::{error, info, warn};
    use oom_watcher_common::OomKillEvent;
    use tokio::io::unix::AsyncFd;

    use crate::{tracepoint, watch::OomEventSource};

    /// The in-cluster adapter for [`OomEventSource`]. Holds the loaded eBPF program (so the
    /// tracepoint stays attached for the source's lifetime) and owns the ring buffer.
    pub struct RingBufSource {
        // Keeps the program attached; never read directly.
        _bpf: Ebpf,
        /// Registered with the reactor so the kernel wakes us on new records instead of us
        /// polling. The window between the kill and the `/proc/<pid>` lookup is a race the
        /// probe cannot win by waiting.
        ring_buf: AsyncFd<RingBuf<MapData>>,
        /// Decoded events not yet handed out. Draining the ring fully before yielding keeps
        /// readiness edges and buffer contents in step: we only clear readiness once the
        /// ring is empty, so no record can sit unnoticed waiting for the next wakeup.
        pending: VecDeque<OomKillEvent>,
        dropped: PerCpuArray<MapData, u64>,
    }

    impl RingBufSource {
        /// Bring up the probe end to end: verify the kernel's tracepoint layout, bump the
        /// memlock rlimit, load the eBPF object, attach to `oom:mark_victim`, and take
        /// ownership of the `EVENTS` ring buffer and the `DROPPED` counter.
        pub fn new() -> Result<Self> {
            // Before anything else: the probe decodes a fixed struct off the trace entry,
            // and attaching to a kernel whose layout differs would silently emit noise
            // rather than fail. See `crate::tracepoint`.
            tracepoint::verify_kernel_layout()?;
            info!("oom:mark_victim tracepoint layout matches the probe");

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

            let events = bpf
                .take_map("EVENTS")
                .ok_or_else(|| anyhow!("Could not find eBPF map 'EVENTS'"))?;
            let ring_buf = AsyncFd::new(RingBuf::try_from(events)?)
                .context("failed to register the EVENTS ring buffer with the tokio reactor")?;

            let dropped = bpf
                .take_map("DROPPED")
                .ok_or_else(|| anyhow!("Could not find eBPF map 'DROPPED'"))?;
            let dropped = PerCpuArray::try_from(dropped)?;

            Ok(Self {
                _bpf: bpf,
                ring_buf,
                pending: VecDeque::new(),
                dropped,
            })
        }
    }

    impl OomEventSource for RingBufSource {
        async fn next(&mut self) -> Option<OomKillEvent> {
            loop {
                if let Some(event) = self.pending.pop_front() {
                    return Some(event);
                }

                // Nothing buffered — park until the kernel says there is something to read.
                let mut guard = match self.ring_buf.readable_mut().await {
                    Ok(guard) => guard,
                    Err(e) => {
                        error!("ring buffer readiness failed: {}", e);
                        return None;
                    }
                };

                let ring_buf = guard.get_inner_mut();
                while let Some(item) = ring_buf.next() {
                    let data: &[u8] = &item;
                    if data.len() >= core::mem::size_of::<OomKillEvent>() {
                        // The eBPF side writes exactly one #[repr(C)] OomKillEvent per entry.
                        let ptr = data.as_ptr() as *const OomKillEvent;
                        self.pending.push_back(unsafe { ptr.read_unaligned() });
                    } else {
                        warn!("Received short event: {} bytes", data.len());
                    }
                }

                // Only now that the ring is drained is it safe to wait for the next edge.
                guard.clear_ready();
            }
        }

        fn dropped_total(&self) -> Option<u64> {
            match self.dropped.get(&0, 0) {
                Ok(per_cpu) => Some(per_cpu.iter().sum()),
                Err(e) => {
                    warn!("could not read the DROPPED counter: {}", e);
                    None
                }
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
