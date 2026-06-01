# Domain Context — ebpf-oom-watcher

Domain vocabulary for the OOM watcher. Architecture reviews and grilling sessions
use these terms exactly.

## Terms

- **OOM kill event** (`OomKillEvent`) — the raw record the kernel emits when it kills
  a process for memory pressure. Captured by the eBPF probe at the `oom:mark_victim`
  tracepoint and shipped to userspace over the ring buffer. Pure numbers + the process
  `comm`; no Kubernetes context.

- **Container identity** (`ContainerIdentity`) — the Kubernetes coordinates of the
  container a killed process belonged to: `namespace`, `pod_name`, `container_name`,
  `container_id`. Resolved from a PID by reading `/proc/<pid>/cgroup` for the container
  id, then matching it against the pods scheduled on this node.

- **Enrichment** — the step that takes a raw **OOM kill event** and a (possibly absent)
  **container identity** and produces an **enriched OOM event**. The single rule it
  encodes: `node_name` is known *iff* this process has a Kubernetes client (i.e. we are
  running in-cluster), regardless of whether the container identity could be resolved.
  Lives in `oom-watcher/src/enrich.rs` as the sole construction site for an enriched
  event.

- **Enriched OOM event** (`EnrichedOomEvent`) — an OOM kill event plus its node name,
  optional container identity fields, and a wall-clock timestamp. The unit recorded as
  Prometheus metrics and logged.

- **Resolution** — the I/O act of turning a PID into a **container identity**. Three
  outcomes: found (`Ok(Some)`), not found (`Ok(None)`), or lookup error (`Err`). The
  watch loop logs the two failure outcomes distinctly, then collapses both to "no
  identity" before handing off to **enrichment**.
