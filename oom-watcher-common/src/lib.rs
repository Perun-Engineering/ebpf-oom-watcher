#![no_std]

#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct OomKillEvent {
    pub pid: u32,
    pub tgid: u32,
    pub comm: [u8; 16],  // Process name (TASK_COMM_LEN)
    pub total_vm: u64,   // Total virtual memory in KB
    pub anon_rss: u64,   // Anonymous RSS in KB  
    pub file_rss: u64,   // File RSS in KB
    pub shmem_rss: u64,  // Shared memory RSS in KB
    pub uid: u32,        // User ID
    pub pgtables: u64,   // Page table size in KB
    pub oom_score_adj: i16, // OOM score adjustment
}
