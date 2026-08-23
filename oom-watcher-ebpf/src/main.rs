#![no_std]
#![no_main]

use aya_ebpf::{
    helpers::{bpf_probe_read_kernel, bpf_probe_read_kernel_str_bytes},
    macros::{map, tracepoint},
    maps::{PerCpuArray, RingBuf},
    programs::TracePointContext,
    EbpfContext,
};
use oom_watcher_common::OomKillEvent;

/// Ring buffer for completed events.
///
/// The kernel requires this to be a power-of-two multiple of `PAGE_SIZE`. 256 KiB satisfies
/// that for every page size in use — including the 64 KiB pages of `CONFIG_ARM64_64K_PAGES`
/// kernels (RHEL 9 aarch64, Oracle UEK aarch64), where anything under 64 KiB makes map
/// creation fail with `EINVAL` and the program never loads. It is also deep enough (~3k
/// events) to ride out an OOM storm between userspace wakeups.
#[map]
static mut EVENTS: RingBuf = RingBuf::with_byte_size(256 * 1024, 0);

/// Events the probe could not enqueue because [`EVENTS`] was full. Surfaced to userspace as
/// `oom_events_dropped_total` — undercounting OOM kills silently is worse than reporting the
/// gap. Per-CPU so the increment needs no atomics.
#[map]
static mut DROPPED: PerCpuArray<u64> = PerCpuArray::with_max_entries(1, 0);

/// The `oom:mark_victim` trace entry, as laid out by the kernel from Linux 6.9 onward
/// (commit "mm: Update mark_victim tracepoints fields"). The offsets below are asserted
/// against the live `/sys/kernel/tracing/events/oom/mark_victim/format` at userspace
/// startup, which refuses to run rather than decode a layout it does not recognise — on
/// kernels before 6.9 the entry is just `common_*` plus `pid`, and reading this struct off
/// it would yield real PIDs paired with adjacent trace-buffer noise.
#[repr(C)]
struct MarkVictimArgs {
    common_type: u16,
    common_flags: u8,
    common_preempt_count: u8,
    common_pid: i32,

    pid: i32,           // offset:8
    comm_data_loc: u32, // offset:12 - __data_loc for comm string
    total_vm: u64,      // offset:16
    anon_rss: u64,      // offset:24
    file_rss: u64,      // offset:32
    shmem_rss: u64,     // offset:40
    uid: u32,           // offset:48
    pgtables: u64,      // offset:56
    oom_score_adj: i16, // offset:64
}

#[tracepoint]
pub fn mark_victim(ctx: TracePointContext) -> u32 {
    // Read tracepoint arguments
    let args: MarkVictimArgs =
        match unsafe { bpf_probe_read_kernel(ctx.as_ptr() as *const MarkVictimArgs) } {
            Ok(args) => args,
            Err(_) => return 0,
        };

    // Extract comm string from __data_loc field
    let mut comm = [0u8; 16];
    let comm_offset = (args.comm_data_loc & 0xFFFF) as usize;
    let comm_ptr = unsafe { (ctx.as_ptr() as *const u8).add(comm_offset) };
    let _ = unsafe { bpf_probe_read_kernel_str_bytes(comm_ptr, &mut comm) };

    let event = OomKillEvent {
        pid: args.pid as u32,
        comm,
        total_vm: args.total_vm,
        anon_rss: args.anon_rss,
        file_rss: args.file_rss,
        shmem_rss: args.shmem_rss,
        uid: args.uid,
        pgtables: args.pgtables,
        oom_score_adj: args.oom_score_adj,
    };

    unsafe {
        // Access the mutable statics through raw pointers to avoid creating shared
        // references to them (see the `static_mut_refs` lint).
        if (*core::ptr::addr_of_mut!(EVENTS))
            .output::<OomKillEvent>(&event, 0)
            .is_err()
        {
            if let Some(dropped) = (*core::ptr::addr_of_mut!(DROPPED)).get_ptr_mut(0) {
                *dropped += 1;
            }
        }
    }

    0
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[link_section = "license"]
#[no_mangle]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";
