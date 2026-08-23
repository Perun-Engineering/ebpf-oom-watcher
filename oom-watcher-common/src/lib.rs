#![no_std]

#[cfg(feature = "user")]
extern crate std;

#[cfg(feature = "user")]
use std::string::String;

/// The raw record the kernel emits at `oom:mark_victim`.
///
/// Field-for-field the payload of that tracepoint as of Linux 6.9, which is the oldest
/// kernel this probe supports; [`crate::OomKillEvent`]'s layout is validated against the
/// live `mark_victim/format` at startup. There is deliberately no `tgid`: the tracepoint
/// carries only the victim's `task->pid`, and `bpf_get_current_pid_tgid()` in this context
/// returns the task that *invoked* the OOM killer, not the victim.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct OomKillEvent {
    /// The victim's `task->pid`. A TID, not necessarily a thread-group leader: the kernel
    /// resolves the victim through `find_lock_task_mm()`, which may pick a thread.
    pub pid: u32,
    pub comm: [u8; 16],     // Process name (TASK_COMM_LEN)
    pub total_vm: u64,      // Total virtual memory in KB
    pub anon_rss: u64,      // Anonymous RSS in KB
    pub file_rss: u64,      // File RSS in KB
    pub shmem_rss: u64,     // Shared memory RSS in KB
    pub uid: u32,           // User ID
    pub pgtables: u64,      // Page table size in KB
    pub oom_score_adj: i16, // OOM score adjustment
}

/// Kubernetes coordinates of the container a killed process belonged to.
///
/// Resolved from a PID by reading `/proc/<pid>/cgroup` for the container id, then
/// matching that id against the pods scheduled on this node.
///
/// `container_id` and `image_id` are taken verbatim from the matched
/// `containerStatuses` entry rather than reconstructed, because that is the same string
/// `kube_pod_container_info` is built from — so joins against it hold by construction.
#[cfg(feature = "user")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerIdentity {
    pub namespace: String,
    pub pod_name: String,
    pub container_name: String,
    /// Runtime-prefixed, as the kubelet reports it: `containerd://<64-hex>`,
    /// `docker://<64-hex>`, `cri-o://<64-hex>`. Not the bare id lifted from the cgroup.
    pub container_id: String,
    /// The image digest the runtime resolved, verbatim — a bare `repo@sha256:…` under
    /// containerd, `docker-pullable://repo@sha256:…` under Docker. Empty for a container
    /// that never started, which cannot be an OOM victim but still parses.
    pub image_id: String,
}

#[cfg(feature = "user")]
#[derive(Clone, Debug)]
pub struct EnrichedOomEvent {
    pub raw_event: OomKillEvent,
    pub node_name: Option<String>,
    pub namespace: Option<String>,
    pub pod_name: Option<String>,
    pub container_name: Option<String>,
    pub container_id: Option<String>,
    pub image_id: Option<String>,
    pub timestamp: u64,
}
