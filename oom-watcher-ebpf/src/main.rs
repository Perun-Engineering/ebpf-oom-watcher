#![no_std]
#![no_main]

use aya_ebpf::{
    helpers::{bpf_get_current_pid_tgid, bpf_probe_read_kernel, bpf_probe_read_kernel_str_bytes},
    macros::{map, tracepoint},
    maps::ring_buf::RingBuf,
    programs::TracePointContext,
    EbpfContext,
};
use oom_watcher_common::OomKillEvent;

#[map]
static mut EVENTS: RingBuf = RingBuf::with_byte_size(4096, 0);

// Tracepoint data structure matching the format from /sys/kernel/tracing/events/oom/mark_victim/format
#[repr(C)]
struct MarkVictimArgs {
    common_type: u16,
    common_flags: u8,
    common_preempt_count: u8,
    common_pid: i32,
    
    pid: i32,              // offset:8
    comm_data_loc: u32,    // offset:12 - __data_loc for comm string
    total_vm: u64,         // offset:16
    anon_rss: u64,         // offset:24
    file_rss: u64,         // offset:32
    shmem_rss: u64,        // offset:40
    uid: u32,              // offset:48
    pgtables: u64,         // offset:56
    oom_score_adj: i16,    // offset:64
}

// Use the oom:mark_victim tracepoint which is available on this kernel
#[tracepoint]
pub fn mark_victim(ctx: TracePointContext) -> u32 {
    let tgid_pid = bpf_get_current_pid_tgid() as u64;
    let current_tgid = (tgid_pid >> 32) as u32;

    // Read tracepoint arguments
    let args: MarkVictimArgs = match unsafe { bpf_probe_read_kernel(ctx.as_ptr() as *const MarkVictimArgs) } {
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
        tgid: current_tgid,
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
        let _ = EVENTS.output(&event, 0);
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